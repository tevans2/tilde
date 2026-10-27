# Known limits

Each formula below fails in KaTeX, the renderer the preview uses. A failed formula shows
as its TeX in red, with the error on hover; the rest of the note still renders. Checked by
`scripts/math-check.js` (see CONTRIBUTING.md).

## MathJax-only

- Cross-references: $$\begin{equation} E = mc^2 \label{eq:emc} \end{equation}$$ and $\eqref{eq:emc}$

## LaTeX packages KaTeX doesn't have

- siunitx: $\SI{9.81}{m/s^2}$, $\num{1e6}$
- TikZ / tikz-cd: $$\begin{tikzcd} A \arrow[r] & B \end{tikzcd}$$
- physics package: $\qty(\frac{a}{b})$, $\dv{f}{x}$ (KaTeX has its `\bra` and `\ket`)

## Turned off on purpose (a note can't link or load)

These show as red text rather than failing:

- $\href{https://example.org}{\text{link}}$, $\url{https://example.org}$, $\includegraphics{x.png}$

## Mistakes in the TeX

- An unmatched brace: $\frac{1}{2$
- A `%` (TeX's comment character) in text: $\text{90% interval}$
- An unknown command: $\foo{x}$
