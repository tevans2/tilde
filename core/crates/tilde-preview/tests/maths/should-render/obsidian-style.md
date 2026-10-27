# Maths as written in Obsidian

Notes in the style of a statistics, linear algebra and theory-of-computation vault. Every
formula here should render.

## Estimation

Suppose $Y_1, \dots, Y_n \overset{\text{iid}}{\sim} \text{Exp}(\theta)$ with mean $\theta$. The
MLE is $\hat\theta = \bar Y$ and $\operatorname{Var}(\hat\theta) = \theta^2 / n$.

$$
\begin{align}
L(\theta) &= \prod_{i=1}^{n} \frac{1}{\theta} e^{-y_i/\theta} \\
&= \theta^{-n} \exp\!\left(-\frac{1}{\theta}\sum_{i=1}^{n} y_i\right)

\\ \therefore \ \ \ell(\theta) &= -n\ln\theta - \frac{1}{\theta}\sum_{i=1}^n y_i
\end{align}
$$

Setting $\frac{d\ell}{d\theta} = 0$ gives $\hat\theta = \frac{1}{n}\sum y_i$. A 90% interval
uses $\chi^2_{2n,\,0.05}$ and costs about $5 to compute by hand, or $0.00 with R.

> [!note] Cramér–Rao
> For an unbiased $T$,
> $$
> \operatorname{Var}(T) \ge \frac{1}{n\,I(\theta)}, \qquad I(\theta) = -\mathbb{E}\!\left[\frac{\partial^2}{\partial\theta^2}\ln f(Y\mid\theta)\right]
> $$

> [!warning]- Common mistake
> $\hat\theta$ unbiased doesn't make $g(\hat\theta)$ unbiased: $\mathbb{E}[\hat\theta^2] = \theta^2 + \operatorname{Var}(\hat\theta)$.

| Theorem | Statement |
|---|---|
| Rao–Blackwell | $\tilde\theta = \mathbb{E}[\hat\theta \mid T]$ has $\text{MSE}(\tilde\theta) \le \text{MSE}(\hat\theta)$ |
| Factorisation | $L = g(T,\theta)\cdot h(\mathbf{x})$, so $T$ is sufficient |
| Conditional | $E[\hat\theta|T]$ with a bare pipe in the cell |
| Display in a cell | $$R^2 = 1 - \frac{\sum (Y_i - \hat Y_i)^2}{\sum (Y_i - \bar Y)^2}$$ |

## Linear algebra

Let $\mathbf{A} \in \mathbb{R}^{m \times n}$ and $\boldsymbol{\beta} = (\beta_0, \beta_1)^\top$.

$$
\mathbf{A} = \begin{bmatrix} 1 & 2 & 0 \\ 0 & 1 & -1 \\ 2 & 5 & -1 \end{bmatrix}
\xrightarrow{R_3 - 2R_1}
\begin{bmatrix} 1 & 2 & 0 \\ 0 & 1 & -1 \\ 0 & 1 & -1 \end{bmatrix}
$$

$$
\hat{\boldsymbol\beta} = (\mathbf{X}^\top\mathbf{X})^{-1}\mathbf{X}^\top\mathbf{y},
\qquad \lVert \mathbf{y} - \mathbf{X}\hat{\boldsymbol\beta} \rVert_2^2 = \min_{\boldsymbol\beta} \lVert \mathbf{y} - \mathbf{X}\boldsymbol\beta \rVert_2^2
$$

- $\operatorname{rank}(\mathbf{A}) + \dim \operatorname{Null}(\mathbf{A}) = n$
- $\text{Col}(\mathbf{A})^\perp = \text{Null}(\mathbf{A}^\top)$
  $$
  \mathbf{x} \in \text{Null}(\mathbf{A}) \iff \mathbf{A}\mathbf{x} = \mathbf{0}
  $$
- A circular buffer drawn as an array:$$
\begin{array}{|c|c|c|c|}
\hline
a & b & c & d \\
\hline
\end{array}
$$

## Piecewise and aligned

$$
f(x) = \begin{cases}
  x^2 & x \ge 0 \\
  -x & x < 0 \quad \text{(the other side)}
\end{cases}
$$

$$
\begin{aligned}
(a+b)^2 &= a^2 + 2ab + b^2 \\
- 2ab &\le a^2 + b^2 % a comment Obsidian ignores
\\
+ \ c &= \underbrace{a + b}_{\text{sum}} \tag{1}
\end{aligned}
$$

## Definitions that carry through the note

$$\newcommand{\E}{\mathbb{E}} \newcommand{\Var}{\operatorname{Var}} \E[X] = \mu$$

Later formulas use them: $\Var(X) = \E[X^2] - \E[X]^2$.

## Theory of computation

$$
\delta(q_0, a) = q_1, \quad L(M) = \{\, w \in \Sigma^* \mid \hat\delta(q_0, w) \in F \,\}
$$

$\text{FIRST}(\alpha)$, $\text{FOLLOW}(A) \ni \$$, and $S \Rightarrow^* \varepsilon$ with
$A \to \alpha \mid \beta$, $\emptyset \subsetneq Q$, $x \wr y$, $R \bowtie S$, $f: A \twoheadrightarrow B$.

## Written by an assistant

The random variable \( X \) has CDF \( F_X(x) = P(X \le x) \), so \( P(a < X \le b) = F_X(b) - F_X(a) \).

## Not maths

Prices like $5 and $10, \$20 escaped, `$code$` in backticks, and \(note\) stay as written.

```python
cost = "$5"  # $$ not maths in code $$
```
