# Maths others write

Formats and packages beyond an Obsidian vault. Every formula here should render.

## GitHub

Inline with backticks: $`\sqrt{x^2+y^2}`$, and a fenced block:

```math
\left( \sum_{k=1}^n a_k b_k \right)^2 \le \left( \sum_{k=1}^n a_k^2 \right) \left( \sum_{k=1}^n b_k^2 \right)
```

## LaTeX brackets

\[
\int_{-\infty}^{\infty} e^{-x^2}\,dx = \sqrt{\pi}
\]

## Physics

$$
\nabla \times \vec{E} = -\frac{\partial \vec{B}}{\partial t}, \qquad
\langle \psi \mid \hat{H} \mid \psi \rangle = E = \bra{\psi} \hat{H} \ket{\psi}, \qquad
\dot{x} = \frac{dx}{dt}, \quad \ddot{x} = -\omega^2 x, \quad \hbar\omega
$$

## Chemistry (mhchem)

$\ce{2H2 + O2 -> 2H2O}$, $\ce{CO2 + C <=> 2CO}$, $\ce{^{227}_{90}Th+}$ and $\pu{123 kJ/mol}$.

## Environments

$$
\begin{gather}
a = b + c \\
d = e
\end{gather}
$$

$$
\begin{alignat}{2}
x &= y + z \quad &\text{by (1)} \\
  &= 2z         &\text{since } y = z
\end{alignat}
$$

$$
\begin{equation*}
\det\begin{vmatrix} a & b \\ c & d \end{vmatrix} = ad - bc,
\quad \begin{pmatrix} 1 & 0 \\ 0 & 1 \end{pmatrix},
\quad \begin{Bmatrix} x \end{Bmatrix},
\quad \begin{smallmatrix} a & b \\ c & d \end{smallmatrix}
\end{equation*}
$$

$$
\begin{CD}
A @>f>> B \\
@VgVV @VVhV \\
C @>>k> D
\end{CD}
$$

## Notation

$$
\sum_{\substack{0 \le i \le m \\ 0 < j < n}} P(i, j), \quad
\overbrace{1 + 2 + \cdots + n}^{n \text{ terms}}, \quad
\cancel{x}\,\bcancel{y}\,\xcancel{z}, \quad
\boxed{E = mc^2}, \quad
\color{teal}{\text{teal}}\ \textcolor{orange}{x}, \quad
\xleftrightarrow[\text{under}]{\text{over}}
$$

$\mathfrak{g}$, $\mathscr{L}$, $\mathcal{O}(n \log n)$, $\mathbb{N} \subset \mathbb{Z} \subset \mathbb{Q}$,
$\operatorname*{arg\,max}_\theta \ell(\theta)$, $\lim\limits_{x\to 0} \frac{\sin x}{x} = 1$,
$\binom{n}{k}$, $\tbinom{n}{k}$, $\phantom{x}$, $\varnothing$, $\iiiint f$, $\textvisiblespace$.

Unicode in maths: $α + β = γ$, $x ≤ y$, and text in other scripts $\text{длина} = 5$.

## Macros

$$\def\R{\mathbb{R}} \def\norm#1{\left\lVert #1 \right\rVert} f: \R^n \to \R, \ \norm{x}_2$$

$$\renewcommand{\R}{\mathbb{R}^+} \R$$

MathJax and LaTeX habits: $\require{cancel} \cancel{x}$, and an operator declared as in a
preamble, $\DeclareMathOperator{\Tr}{Tr} \DeclareMathOperator*{\argmin}{arg\,min} \Tr A = \argmin_x f$.

## Numbering

$$ x^2 + y^2 = r^2 \tag{circle} $$
