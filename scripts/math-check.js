#!/usr/bin/env node
// Checks the maths in a folder of Markdown notes with the KaTeX the app bundles, set up
// exactly as the preview renders it (App/Sources/Tilde/Resources/katex/tilde-math.js).
// The formulas come from the Rust parser, so this sees what the preview sees:
//
//   cargo run -q --manifest-path core/Cargo.toml -p tilde-preview --example math_survey -- <folder> \
//     | node scripts/math-check.js
//
// Prints how many formulas render and the failures, grouped by error, with examples.
const fs = require("fs");
const path = require("path");
const vm = require("vm");

const dir = path.join(__dirname, "../App/Sources/Tilde/Resources/katex");
const context = vm.createContext({ console });
context.globalThis = context;
for (const f of ["katex.min.js", "contrib/mhchem.min.js", "tilde-math.js"]) {
  vm.runInContext(fs.readFileSync(path.join(dir, f), "utf8"), context, { filename: f });
}
const { katex, tildeMath } = context;

const lines = fs.readFileSync(0, "utf8").split("\n").filter(Boolean);
let total = 0;
let failed = 0;
const groups = new Map();
let file = null;
let macros = null;
for (const line of lines) {
  const m = JSON.parse(line);
  // \newcommand carries through a note, as it does in the preview.
  if (m.file !== file) {
    file = m.file;
    macros = Object.assign({}, tildeMath.builtins);
  }
  total++;
  try {
    katex.renderToString(tildeMath.prepare(m.tex), tildeMath.options(m.display, macros));
  } catch (e) {
    failed++;
    const message = String(e.message).replace(/^KaTeX parse error: /, "");
    const key = (message.match(/Undefined control sequence: (\\\S+)/) || [])[0] || message.replace(/ at position \d+[\s\S]*$/, "").slice(0, 80);
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key).push(`${m.file}: ${m.tex.replace(/\s+/g, " ").slice(0, 100)}`);
  }
}
console.log(`${total} formulas, ${total - failed} render, ${failed} fail (${((100 * failed) / Math.max(total, 1)).toFixed(2)}%)`);
for (const [key, list] of [...groups].sort((a, b) => b[1].length - a[1].length)) {
  console.log(`\n${String(list.length).padStart(4)}  ${key}`);
  for (const e of list.slice(0, 3)) console.log(`        ${e}`);
}
process.exitCode = failed > 0 ? 1 : 0;
