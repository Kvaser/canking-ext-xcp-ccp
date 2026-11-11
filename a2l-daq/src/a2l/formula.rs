//! A small self-contained expression compiler for `COMPU_METHOD FORM` formulas.
//!
//! Reduces a formula string like `"(X1/365-0.5)+1970"` to a linear `factor * Xn + offset` form,
//! when it is in fact linear in exactly one input variable. Used to fold a virtual measurement's `FORM`
//! conversion into a plain linear one so it can use the real measurement's ecu address.

/// Formula tokens
#[derive(Debug, Clone, PartialEq)]
enum FormulaToken {
    /// A numeric literal
    Number(f64),
    /// An identifier, usually `Xn`
    Ident(String),
    /// An operator: `+`, `-`, `*`, `/`
    Op(char),
    /// An open parenthesis `(`
    Open,
    /// A close parenthesis `)`
    Close,
}

/// A linear formula reduced to `factor * Xn + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Linear {
    slope: f64,
    constant: f64,
}

impl Linear {
    const fn constant(v: f64) -> Self {
        Linear {
            slope: 0.0,
            constant: v,
        }
    }

    fn is_constant(&self) -> bool {
        self.slope == 0.0
    }
}

/// Reduce a `FORM` formula to `(variable, factor, offset)` such that `physical = factor * X<variable> + offset`.
pub(super) fn linear_from_formula_var(formula: &str) -> Option<(Option<usize>, f64, f64)> {
    let tokens = tokenize_formula(formula)?;
    let variable = sole_formula_var(&tokens)?;
    let mut cursor = 0usize;
    let result = parse_formula_expr(&tokens, &mut cursor)?;
    if cursor != tokens.len() {
        return None;
    }
    Some((variable, result.slope, result.constant))
}

/// The 1-based index of `name` when it is an `Xn` reference.
fn formula_var_index(name: &str) -> Option<usize> {
    name.strip_prefix('X')
        .or_else(|| name.strip_prefix('x'))
        .and_then(|digits| match digits {
            "" => Some(1),
            _ => digits.parse::<usize>().ok(),
        })
        .filter(|&index| index > 0)
}

/// The single `Xn` index every identifier in `tokens` refers to.
fn sole_formula_var(tokens: &[FormulaToken]) -> Option<Option<usize>> {
    let mut seen: Option<usize> = None;
    for token in tokens {
        let FormulaToken::Ident(name) = token else {
            continue;
        };
        let index = formula_var_index(name)?;
        match seen {
            Some(previous) if previous != index => return None,
            _ => seen = Some(index),
        }
    }
    Some(seen)
}
/// Tokenize a formula string into a sequence of formula tokens.
fn tokenize_formula(src: &str) -> Option<Vec<FormulaToken>> {
    let mut out = Vec::new();
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c.is_ascii_digit() || c == '.' {
            let start = i;
            if c == '0' && matches!(chars.get(i + 1), Some('x') | Some('X')) {
                i += 2;
                while i < chars.len() && chars[i].is_ascii_hexdigit() {
                    i += 1;
                }
                let text: String = chars[start + 2..i].iter().collect();
                out.push(FormulaToken::Number(
                    u64::from_str_radix(&text, 16).ok()? as f64
                ));
                continue;
            }
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let text: String = chars[start..i].iter().collect();
            out.push(FormulaToken::Number(text.parse().ok()?));
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(FormulaToken::Ident(chars[start..i].iter().collect()));
        } else {
            i += 1;
            match c {
                '(' => out.push(FormulaToken::Open),
                ')' => out.push(FormulaToken::Close),
                '+' | '-' | '*' | '/' => out.push(FormulaToken::Op(c)),
                _ => return None,
            }
        }
    }
    Some(out)
}

/// Parse a formula expression into a linear form, returning `None` if it is not linear in exactly one variable.
fn parse_formula_expr(tokens: &[FormulaToken], cursor: &mut usize) -> Option<Linear> {
    let mut acc = parse_formula_term(tokens, cursor)?;
    while let Some(FormulaToken::Op(op @ ('+' | '-'))) = tokens.get(*cursor) {
        let op = *op;
        *cursor += 1;
        let rhs = parse_formula_term(tokens, cursor)?;
        let sign = if op == '+' { 1.0 } else { -1.0 };
        acc = Linear {
            slope: acc.slope + sign * rhs.slope,
            constant: acc.constant + sign * rhs.constant,
        };
    }
    Some(acc)
}

/// Parse a formula term into a linear form, returning `None` if it is not linear in exactly one variable.
fn parse_formula_term(tokens: &[FormulaToken], cursor: &mut usize) -> Option<Linear> {
    let mut acc = parse_formula_unary(tokens, cursor)?;
    while let Some(FormulaToken::Op(op @ ('*' | '/'))) = tokens.get(*cursor) {
        let op = *op;
        *cursor += 1;
        let rhs = parse_formula_unary(tokens, cursor)?;
        acc = if op == '*' {
            match (acc.is_constant(), rhs.is_constant()) {
                (true, _) => Linear {
                    slope: rhs.slope * acc.constant,
                    constant: rhs.constant * acc.constant,
                },
                (_, true) => Linear {
                    slope: acc.slope * rhs.constant,
                    constant: acc.constant * rhs.constant,
                },
                _ => return None,
            }
        } else {
            if !rhs.is_constant() || rhs.constant == 0.0 {
                return None;
            }
            Linear {
                slope: acc.slope / rhs.constant,
                constant: acc.constant / rhs.constant,
            }
        };
    }
    Some(acc)
}

/// Parse a formula unary expression into a linear form, returning `None` if it is not linear in exactly one variable.
fn parse_formula_unary(tokens: &[FormulaToken], cursor: &mut usize) -> Option<Linear> {
    if let Some(FormulaToken::Op(op @ ('+' | '-'))) = tokens.get(*cursor) {
        let negate = *op == '-';
        *cursor += 1;
        let inner = parse_formula_unary(tokens, cursor)?;
        return Some(if negate {
            Linear {
                slope: -inner.slope,
                constant: -inner.constant,
            }
        } else {
            inner
        });
    }
    parse_formula_primary(tokens, cursor)
}

/// Parse a formula primary expression into a linear form, returning `None` if it is not linear in exactly one variable.
fn parse_formula_primary(tokens: &[FormulaToken], cursor: &mut usize) -> Option<Linear> {
    match tokens.get(*cursor)? {
        FormulaToken::Number(n) => {
            *cursor += 1;
            Some(Linear::constant(*n))
        }
        FormulaToken::Ident(name) => {
            *cursor += 1;
            // Which `Xn` this is has already been settled by `sole_formula_var` so every variable here is the same one.
            formula_var_index(name).map(|_| Linear {
                slope: 1.0,
                constant: 0.0,
            })
        }
        FormulaToken::Open => {
            *cursor += 1;
            let inner = parse_formula_expr(tokens, cursor)?;
            if tokens.get(*cursor) != Some(&FormulaToken::Close) {
                return None;
            }
            *cursor += 1;
            Some(inner)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn linear_formula_reports_which_input_it_reduces_against() {
        use super::linear_from_formula_var as lin;

        assert_eq!(lin("X1*2"), Some((Some(1), 2.0, 0.0)));
        assert_eq!(lin("X3/4+1"), Some((Some(3), 0.25, 1.0)));
        assert_eq!(lin("x2"), Some((Some(2), 1.0, 0.0)));
        assert_eq!(lin("42"), Some((None, 0.0, 42.0)));
        assert_eq!(lin("X1+X2"), None);
        assert_eq!(lin("X1*X1"), None);
        assert_eq!(lin("Rpm*2"), None);
    }
}
