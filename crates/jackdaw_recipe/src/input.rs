//! A recipe's named inputs: their kind, default and range, and the check a
//! value given for one passes before anything is built.

use crate::expr::Value;

/// The input every recipe has: the seed `rand` draws from.
pub const SEED_INPUT: &str = "seed";

/// What an input holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Float,
    Int,
    Bool,
    Vec2,
    Vec3,
    Text,
}

impl InputKind {
    pub const NAMES: [&'static str; 6] = ["float", "int", "bool", "vec2", "vec3", "text"];

    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "float" => Ok(Self::Float),
            "int" => Ok(Self::Int),
            "bool" => Ok(Self::Bool),
            "vec2" => Ok(Self::Vec2),
            "vec3" => Ok(Self::Vec3),
            "text" => Ok(Self::Text),
            other => Err(format!(
                "`{other}` is not an input kind; one of {}",
                Self::NAMES.join(", ")
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Int => "int",
            Self::Bool => "bool",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Text => "text",
        }
    }

    fn components(self) -> Option<usize> {
        match self {
            Self::Vec2 => Some(2),
            Self::Vec3 => Some(3),
            _ => None,
        }
    }
}

/// One input, as a recipe declares it.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSpec {
    pub name: String,
    pub kind: InputKind,
    pub default: Value,
    /// Inclusive bounds on a number, or on each component of a vector.
    pub range: Option<(f64, f64)>,
}

impl InputSpec {
    /// Read a declaration, its default given as text and its range as an
    /// empty list or `[min, max]`.
    pub fn new(name: &str, kind: &str, default: &str, range: &[f64]) -> Result<Self, String> {
        if name.is_empty()
            || name.starts_with(|c: char| c.is_ascii_digit())
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            return Err(format!(
                "`{name}` is not an input name; use ASCII letters, digits and `_`, not starting \
                 with a digit"
            ));
        }
        if name == SEED_INPUT {
            return Err(format!(
                "`{SEED_INPUT}` is built in; every recipe has it, so it is not declared"
            ));
        }
        let kind = InputKind::parse(kind).map_err(|why| format!("input `{name}`: {why}"))?;
        let range = match range {
            [] => None,
            [min, max] if min <= max => Some((*min, *max)),
            _ => {
                return Err(format!(
                    "input `{name}`: a range is [min, max] with min no greater than max"
                ));
            }
        };
        if range.is_some() && matches!(kind, InputKind::Bool | InputKind::Text) {
            return Err(format!("input `{name}`: a {} takes no range", kind.name()));
        }
        let mut spec = Self {
            name: name.to_string(),
            kind,
            default: Value::Number(0.0),
            range,
        };
        spec.default = spec
            .read_text(default)
            .map_err(|why| format!("the default of {why}"))?;
        Ok(spec)
    }

    /// Check a value given for this input, answering it as the kind holds it.
    pub fn check(&self, value: Value) -> Result<Value, String> {
        let wrong = |given: &Value| {
            format!(
                "`{}` is {}, not {}",
                self.name,
                self.describe(),
                describe_value(given)
            )
        };
        let checked = match (self.kind, value) {
            (InputKind::Float, Value::Number(n)) if n.is_finite() => Value::Number(n),
            (InputKind::Int, Value::Number(n)) if n.is_finite() && n.fract() == 0.0 => {
                Value::Number(n)
            }
            (InputKind::Bool, Value::Bool(b)) => Value::Bool(b),
            (InputKind::Text, Value::Text(text)) => Value::Text(text),
            (kind @ (InputKind::Vec2 | InputKind::Vec3), Value::List(items))
                if Some(items.len()) == kind.components()
                    && items
                        .iter()
                        .all(|item| matches!(item, Value::Number(n) if n.is_finite())) =>
            {
                Value::List(items)
            }
            (_, given) => return Err(wrong(&given)),
        };
        if let Some((min, max)) = self.range {
            let numbers: Vec<f64> = match &checked {
                Value::Number(n) => vec![*n],
                Value::List(items) => items
                    .iter()
                    .filter_map(|item| match item {
                        Value::Number(n) => Some(*n),
                        _ => None,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            if numbers.iter().any(|n| *n < min || *n > max) {
                return Err(wrong(&checked));
            }
        }
        Ok(checked)
    }

    /// Read a value written as text, as a default or a stored input is.
    pub fn read_text(&self, text: &str) -> Result<Value, String> {
        let text = text.trim();
        let value = match self.kind {
            InputKind::Text => Value::Text(text.to_string()),
            InputKind::Bool => match text {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(format!("`{}`: `{text}` is not true or false", self.name)),
            },
            InputKind::Float | InputKind::Int => match text.parse::<f64>() {
                Ok(n) => Value::Number(n),
                Err(_) => return Err(format!("`{}`: `{text}` is not a number", self.name)),
            },
            InputKind::Vec2 | InputKind::Vec3 => {
                let inner = text
                    .strip_prefix('[')
                    .and_then(|rest| rest.strip_suffix(']'))
                    .unwrap_or(text);
                let parts: Result<Vec<Value>, _> = inner
                    .split(',')
                    .map(|part| part.trim().parse::<f64>().map(Value::Number))
                    .collect();
                match parts {
                    Ok(parts) => Value::List(parts),
                    Err(_) => {
                        return Err(format!(
                            "`{}`: `{text}` is not a list of numbers",
                            self.name
                        ));
                    }
                }
            }
        };
        self.check(value)
    }

    /// What the input takes, for a refusal: `an int from 1 to 4`.
    pub fn describe(&self) -> String {
        let kind = match self.kind {
            InputKind::Float => "a number",
            InputKind::Int => "a whole number",
            InputKind::Bool => "true or false",
            InputKind::Vec2 => "a vector of 2 numbers",
            InputKind::Vec3 => "a vector of 3 numbers",
            InputKind::Text => "text",
        };
        match self.range {
            Some((min, max)) if self.kind.components().is_some() => {
                format!("{kind}, each from {} to {}", spell(min), spell(max))
            }
            Some((min, max)) => format!("{kind} from {} to {}", spell(min), spell(max)),
            None => kind.to_string(),
        }
    }
}

/// A value written back as text, the way [`InputSpec::read_text`] reads it.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Number(n) => spell(*n),
        Value::Bool(b) => b.to_string(),
        Value::Text(text) | Value::Ref(text) => text.clone(),
        Value::List(items) => items.iter().map(value_text).collect::<Vec<_>>().join(", "),
    }
}

fn spell(n: f64) -> String {
    if n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

fn describe_value(value: &Value) -> String {
    match value {
        Value::Text(text) => format!("the text `{text}`"),
        other => format!("`{}`", value_text(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floors() -> InputSpec {
        InputSpec::new("floors", "int", "1", &[1.0, 4.0]).expect("valid")
    }

    #[test]
    fn a_declaration_reads_its_default_as_its_kind() {
        assert_eq!(floors().default, Value::Number(1.0));
        let size = InputSpec::new("size", "vec3", "1, 2, 3", &[]).expect("valid");
        assert_eq!(
            size.default,
            Value::List(vec![
                Value::Number(1.0),
                Value::Number(2.0),
                Value::Number(3.0)
            ])
        );
        let open = InputSpec::new("open", "bool", "true", &[]).expect("valid");
        assert_eq!(open.default, Value::Bool(true));
        let material = InputSpec::new("material", "text", "", &[]).expect("valid");
        assert_eq!(material.default, Value::Text(String::new()));
    }

    #[test]
    fn a_bad_declaration_is_refused_with_its_name() {
        for (name, kind, default, range) in [
            ("2x", "int", "1", &[][..]),
            ("seed", "int", "1", &[]),
            ("floors", "integer", "1", &[]),
            ("floors", "int", "9", &[1.0, 4.0]),
            ("floors", "int", "1", &[4.0, 1.0]),
            ("open", "bool", "true", &[0.0, 1.0]),
            ("size", "vec3", "1, 2", &[]),
        ] {
            assert!(
                InputSpec::new(name, kind, default, range).is_err(),
                "{name} {kind} {default}"
            );
        }
    }

    #[test]
    fn a_value_out_of_range_or_of_the_wrong_kind_names_the_input() {
        let spec = floors();
        assert_eq!(spec.check(Value::Number(3.0)), Ok(Value::Number(3.0)));
        for given in [
            Value::Number(9.0),
            Value::Number(2.5),
            Value::Text("two".into()),
            Value::Bool(true),
        ] {
            let err = spec.check(given.clone()).expect_err("refused");
            assert!(err.contains("`floors`"), "{err}");
            assert!(err.contains("from 1 to 4"), "{err}");
        }
        let size = InputSpec::new("size", "vec3", "1, 1, 1", &[0.0, 10.0]).expect("valid");
        let err = size
            .check(Value::List(vec![
                Value::Number(1.0),
                Value::Number(20.0),
                Value::Number(1.0),
            ]))
            .expect_err("out of range");
        assert!(err.contains("each from 0 to 10"), "{err}");
    }

    #[test]
    fn a_value_reads_back_from_the_text_it_writes() {
        let size = InputSpec::new("size", "vec3", "1, 2.5, 3", &[]).expect("valid");
        let text = value_text(&size.default);
        assert_eq!(text, "1, 2.5, 3");
        assert_eq!(size.read_text(&text), Ok(size.default.clone()));
        assert_eq!(size.read_text("[1, 2.5, 3]"), Ok(size.default));
    }
}
