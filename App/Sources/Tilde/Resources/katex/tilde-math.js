// Renders the maths in a Markdown preview with KaTeX. The Markdown renderer
// (core/crates/tilde-preview/src/math.rs) leaves each formula's TeX in a
// `.math-inline` or `.math-display` span; this replaces them with KaTeX's output.
// The app runs it in an isolated script world (the page's own scripts are off);
// scripts/math-check.js runs the same code over whole folders of notes.
(function (root) {
  // Commands KaTeX lacks that turn up in notes written for MathJax (Obsidian).
  const builtins = {
    "\\textvisiblespace": "\\text{␣}",
    "\\iiiint": "\\int\\!\\!\\!\\int\\!\\!\\!\\int\\!\\!\\!\\int",
  };

  // Obsidian (MathJax's defaults) and GitHub don't number equations unless asked with
  // \tag, so neither does the preview: align, equation… become their unnumbered forms.
  // MathJax and LaTeX habits KaTeX lacks: \require{…} (MathJax's extension loader) does
  // nothing here, and \DeclareMathOperator becomes a \def (which, unlike \newcommand, can
  // replace an operator KaTeX already has, like \argmin).
  function prepare(tex) {
    return tex
      .replace(/\\(begin|end)\{(align|alignat|equation|gather|multline)\}/g, "\\$1{$2*}")
      .replace(/\\require\{[^}]*\}/g, "")
      .replace(/\\DeclareMathOperator(\*?)\s*\{?(\\[A-Za-z]+)\}?\s*\{([^}]*)\}/g,
        (_, star, name, text) => `\\def${name}{\\operatorname${star}{${text}}}`);
  }

  function options(display, macros) {
    return {
      displayMode: display,
      throwOnError: true,
      // MathJax (Obsidian) accepts what KaTeX's strict mode warns about, like Unicode text.
      strict: "ignore",
      // No \href, \includegraphics or \htmlClass: a note can't load or link anything.
      trust: false,
      // \newcommand in one formula holds for the rest of the note.
      globalGroup: true,
      macros,
      maxExpand: 2000,
      output: "html",
    };
  }

  // Renders every formula in the document; a formula KaTeX can't render shows its TeX,
  // marked, with the error as a tooltip. Returns how many failed.
  function renderAll(doc, katex) {
    const macros = Object.assign({}, builtins);
    let failed = 0;
    for (const el of doc.querySelectorAll(".math-inline, .math-display")) {
      const tex = el.textContent;
      try {
        katex.render(prepare(tex), el, options(el.classList.contains("math-display"), macros));
      } catch (e) {
        failed++;
        el.classList.add("math-error");
        el.title = String(e.message).replace(/^KaTeX parse error: /, "");
        el.textContent = tex;
      }
    }
    return failed;
  }

  root.tildeMath = { builtins, prepare, options, renderAll };
})(globalThis);
