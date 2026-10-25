-- Tilde's folder-as-text buffer (docs/SPEC.md §6.9). Tilde loads this over RPC as
-- `package.loaded.tilde` rather than from 'runtimepath', which plugin managers reset.
--
-- Each line is `/<id> name` (folders end in `/`). The `/<id> ` prefix is concealed and the
-- cursor is kept out of it; Tilde diffs the lines on :w. Users can adjust the buffer with
-- a `FileType tilde` autocommand (for example to turn off format-on-save).

local M = {}
local ns = vim.api.nvim_create_namespace('tilde')
local uv = vim.uv or vim.loop
local PREVIEW_DELAY = 100

-- The open session: { buf, chan, quiet, timer }.
local s = nil

local function prefix_len(line)
  local p = line:match('^/%d+ ')
  return p and #p or 0
end

local function buf_lines(buf)
  return vim.api.nvim_buf_get_lines(buf, 0, -1, false)
end

local function keys(k)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(k, true, false, true), 'n', false)
end

-- Conceals prefixes and colours folders. Redone on every change: lines carry their IDs
-- through yank and paste, and extmarks don't.
local function decorate(buf)
  vim.api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  for i, line in ipairs(buf_lines(buf)) do
    local p = prefix_len(line)
    if p > 0 then
      vim.api.nvim_buf_set_extmark(buf, ns, i - 1, 0, { end_col = p, conceal = '' })
    end
    if #line > p + 1 and line:sub(-1) == '/' then
      vim.api.nvim_buf_set_extmark(buf, ns, i - 1, p, { end_col = #line, hl_group = 'Directory' })
    end
  end
end

local function clamp_cursor()
  local win = vim.api.nvim_get_current_win()
  local pos = vim.api.nvim_win_get_cursor(win)
  local row, col = pos[1], pos[2]
  local line = vim.api.nvim_buf_get_lines(0, row - 1, row, false)[1] or ''
  local p = prefix_len(line)
  if col < p then
    vim.api.nvim_win_set_cursor(win, { row, p })
  end
end

local function schedule_preview()
  local cur = s
  if not cur then return end
  cur.timer:stop()
  cur.timer:start(PREVIEW_DELAY, 0, vim.schedule_wrap(function()
    if s == cur and vim.api.nvim_buf_is_valid(cur.buf) then
      vim.rpcnotify(cur.chan, 'tilde_preview', buf_lines(cur.buf))
    end
  end))
end

local function write(buf)
  if not s or s.buf ~= buf then return end
  local ok, res = pcall(vim.rpcrequest, s.chan, 'tilde_apply', buf_lines(buf))
  if not ok then
    vim.notify('Tilde: ' .. tostring(res), vim.log.levels.ERROR)
  elseif type(res) == 'string' then
    vim.notify(res, vim.log.levels.ERROR)
  else
    -- Tilde applies it and closes the buffer, or calls M.error.
    vim.bo[buf].modified = false
  end
end

-- Byte length of a name up to its extension (or trailing `/`).
local function stem_len(name)
  if name:sub(-1) == '/' then return #name - 1 end
  local dot = name:match('^.+()%.[^.]*$')
  return dot and dot - 1 or #name
end

local function set_quit_commands(buf)
  vim.api.nvim_buf_create_user_command(buf, 'TildeCancel', function() M.cancel() end, { bang = true })
  local function abbrev(cmd, to)
    vim.cmd(('cnoreabbrev <buffer> <expr> %s getcmdtype() ==# ":" && getcmdline() ==# "%s" ? "%s" : "%s"')
      :format(cmd, cmd, to, cmd))
  end
  for _, cmd in ipairs({ 'q', 'quit', 'clo', 'close' }) do abbrev(cmd, 'TildeCancel') end
  for _, cmd in ipairs({ 'wq', 'x', 'xit', 'exi', 'exit' }) do abbrev(cmd, 'write') end
  vim.keymap.set('n', 'ZZ', '<Cmd>write<CR>', { buffer = buf })
  vim.keymap.set('n', 'ZQ', '<Cmd>TildeCancel<CR>', { buffer = buf })
end

-- Opens a session. opts: { chan, dir, lines, line (1-based), mode = 'edit'|'rename'|'new' }.
function M.open(opts)
  M.close()
  local buf = vim.api.nvim_create_buf(true, false)
  local old = vim.fn.bufnr('tilde://' .. opts.dir)
  if old > 0 then vim.api.nvim_buf_delete(old, { force = true }) end
  vim.bo[buf].swapfile = false
  vim.bo[buf].buftype = 'acwrite'
  vim.bo[buf].bufhidden = 'wipe'
  vim.api.nvim_buf_set_name(buf, 'tilde://' .. opts.dir)
  -- Loading the text isn't an undoable change.
  local undolevels = vim.api.nvim_get_option_value('undolevels', { buf = buf })
  vim.bo[buf].undolevels = -1
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, opts.lines)
  vim.bo[buf].undolevels = undolevels
  vim.bo[buf].modified = false

  s = { buf = buf, chan = opts.chan, quiet = false, timer = uv.new_timer() }
  local win = vim.api.nvim_get_current_win()
  vim.api.nvim_win_set_buf(win, buf)
  vim.api.nvim_set_option_value('conceallevel', 3, { scope = 'local', win = win })
  vim.api.nvim_set_option_value('concealcursor', 'nvic', { scope = 'local', win = win })
  vim.api.nvim_set_option_value('wrap', false, { scope = 'local', win = win })
  decorate(buf)
  set_quit_commands(buf)

  local group = vim.api.nvim_create_augroup('tilde_session', { clear = true })
  vim.api.nvim_create_autocmd({ 'TextChanged', 'TextChangedI', 'TextChangedP' }, {
    group = group, buffer = buf,
    callback = function() decorate(buf); schedule_preview() end,
  })
  vim.api.nvim_create_autocmd({ 'CursorMoved', 'CursorMovedI' }, { group = group, buffer = buf, callback = clamp_cursor })
  vim.api.nvim_create_autocmd('BufWriteCmd', { group = group, buffer = buf, callback = function() write(buf) end })
  vim.api.nvim_create_autocmd('BufWipeout', {
    group = group, buffer = buf,
    callback = function()
      local cur = s
      if cur and cur.buf == buf then
        s = nil
        cur.timer:close()
        if not cur.quiet then vim.rpcnotify(cur.chan, 'tilde_cancel') end
      end
    end,
  })
  -- Last, so a user's `FileType tilde` autocommand can change any of the above.
  vim.bo[buf].filetype = 'tilde'

  local row = math.max(1, math.min(opts.line or 1, vim.api.nvim_buf_line_count(buf)))
  local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ''
  local p = prefix_len(line)
  vim.api.nvim_win_set_cursor(win, { row, p })
  keys('<C-\\><C-n>')
  if opts.mode == 'rename' then
    vim.keymap.set('i', '<CR>', '<Esc><Cmd>write<CR>', { buffer = buf })
    local n = vim.fn.strchars(line:sub(p + 1, p + stem_len(line:sub(p + 1))))
    -- Select the stem in select mode, so typing replaces it.
    if n > 0 then keys('v' .. (n > 1 and (n - 1) .. 'l' or '') .. '<C-g>') else keys('a') end
  elseif opts.mode == 'new' then
    vim.keymap.set('i', '<CR>', '<Esc><Cmd>write<CR>', { buffer = buf })
    keys(#opts.lines == 0 and 'i' or 'o')
  end
end

-- Closes the session without reporting a cancel (Tilde applied it or cancelled itself).
function M.close()
  local cur = s
  if not cur then return end
  cur.quiet = true
  if vim.api.nvim_buf_is_valid(cur.buf) then
    keys('<C-\\><C-n>')
    vim.api.nvim_buf_delete(cur.buf, { force = true })
  end
  s = nil
end

-- Discards the session (:q) and tells Tilde.
function M.cancel()
  local cur = s
  if not cur then return end
  M.close()
  vim.rpcnotify(cur.chan, 'tilde_cancel')
end

-- Tilde's ⌘S.
function M.write()
  if s and vim.api.nvim_buf_is_valid(s.buf) then
    vim.api.nvim_buf_call(s.buf, function() vim.cmd('write') end)
  end
end

-- Applying failed: say why and keep the buffer.
function M.error(message)
  vim.notify(message, vim.log.levels.ERROR)
  if s and vim.api.nvim_buf_is_valid(s.buf) then vim.bo[s.buf].modified = true end
end

-- Lines Tilde can't apply: { { line (0-based), message }, ... }.
function M.diagnostics(items)
  if not s or not vim.api.nvim_buf_is_valid(s.buf) then return end
  local d = {}
  for _, e in ipairs(items) do
    d[#d + 1] = { lnum = e[1], col = 0, message = e[2], severity = vim.diagnostic.severity.ERROR, source = 'tilde' }
  end
  vim.diagnostic.set(ns, s.buf, d)
end

return M
