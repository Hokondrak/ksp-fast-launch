//! Expressions in Mono's AOT assembly: sums and differences of symbols, `.` (the current location), integers and
//! `symbol@IMGREL` (the symbol's address relative to the image base). Parentheses and other operators are not
//! used by Mono and are rejected.

/// A term's kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Term {
    /// A symbol's address (image base + RVA once linked).
    Symbol(String),
    /// A symbol's RVA.
    ImageRelative(String),
    /// The address of the item being emitted.
    Here,
}

/// `constant + sum(coefficient * term)`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expr {
    pub constant: i64,
    pub terms: Vec<(i64, Term)>,
}

impl Expr {
    pub fn is_constant(&self) -> bool {
        self.terms.is_empty()
    }
}

pub fn parse(text: &str) -> Result<Expr, String> {
    let s = text.trim();
    if s.is_empty() {
        return Err("empty expression".into());
    }
    let mut e = Expr::default();
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut sign = 1i64;
    let mut expect_operand = true;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '+' || c == '-' {
            if expect_operand {
                // unary sign
                if c == '-' {
                    sign = -sign;
                }
            } else {
                sign = if c == '-' { -1 } else { 1 };
                expect_operand = true;
            }
            i += 1;
            continue;
        }
        if !expect_operand {
            return Err(format!("unexpected '{}' in expression '{}'", c, s));
        }
        let start = i;
        if c.is_ascii_digit() {
            while i < bytes.len() && (bytes[i] as char).is_ascii_alphanumeric() {
                i += 1;
            }
            e.constant = e.constant.wrapping_add(sign.wrapping_mul(number(&s[start..i])?));
        } else if c == '.' && (i + 1 == bytes.len() || !is_symbol_char(bytes[i + 1] as char)) {
            i += 1;
            e.terms.push((sign, Term::Here));
        } else if is_symbol_start(c) {
            while i < bytes.len() && is_symbol_char(bytes[i] as char) {
                i += 1;
            }
            let name = &s[start..i];
            if let Some(sym) = name.strip_suffix("@IMGREL") {
                e.terms.push((sign, Term::ImageRelative(sym.to_string())));
            } else if name.contains('@') {
                return Err(format!("unsupported relocation operator in '{}'", name));
            } else {
                e.terms.push((sign, Term::Symbol(name.to_string())));
            }
        } else {
            return Err(format!("unexpected '{}' in expression '{}'", c, s));
        }
        sign = 1;
        expect_operand = false;
    }
    if expect_operand {
        return Err(format!("expression '{}' ends with an operator", s));
    }
    Ok(e)
}

fn is_symbol_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '.' || c == '$'
}

fn is_symbol_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$' || c == '@'
}

fn number(t: &str) -> Result<i64, String> {
    let v = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        u64::from_str_radix(h, 16).map(|v| v as i64)
    } else if t.len() > 1 && t.starts_with('0') && t.bytes().all(|b| b.is_ascii_digit()) {
        u64::from_str_radix(&t[1..], 8).map(|v| v as i64)
    } else {
        t.parse::<i64>().or_else(|_| t.parse::<u64>().map(|v| v as i64))
    };
    v.map_err(|_| format!("bad number '{}'", t))
}

/// Splits a directive's operand list on commas that are outside string literals.
pub fn split_operands(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        match c {
            '\\' if in_str && !escaped => escaped = true,
            '"' if !escaped => in_str = !in_str,
            ',' if !in_str => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => escaped = false,
        }
        if c != '\\' {
            escaped = false;
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() || !out.is_empty() {
        out.push(last);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_what_mono_writes() {
        assert_eq!(parse("142").unwrap(), Expr { constant: 142, terms: vec![] });
        assert_eq!(parse("-5").unwrap().constant, -5);
        assert_eq!(parse("0x1F").unwrap().constant, 31);
        let e = parse("mono_aot_X_got - . + 196").unwrap();
        assert_eq!(e.constant, 196);
        assert_eq!(e.terms, vec![(1, Term::Symbol("mono_aot_X_got".into())), (-1, Term::Here)]);
        let e = parse(".Lme_0 - .Lm_0").unwrap();
        assert_eq!(e.terms, vec![(1, Term::Symbol(".Lme_0".into())), (-1, Term::Symbol(".Lm_0".into()))]);
        assert_eq!(parse(".Lm_0@IMGREL").unwrap().terms, vec![(1, Term::ImageRelative(".Lm_0".into()))]);
        assert!(parse("a * 2").is_err());
        assert!(parse("a@GOTPCREL").is_err());
        assert!(parse("a -").is_err());
    }

    #[test]
    fn splits_operands_outside_strings() {
        assert_eq!(split_operands("1, 2,3"), vec!["1", "2", "3"]);
        assert_eq!(split_operands("\"a,b\", 4"), vec!["\"a,b\"", "4"]);
        assert_eq!(split_operands("\"x\\\",y\""), vec!["\"x\\\",y\""]);
    }
}
