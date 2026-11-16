//! Conditions on rows: which ones a change is for, when its author
//! can't know.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use super::{ColumnId, Row, Value};

/// Picks rows by what they hold: every test has to pass. With no
/// tests, it picks every row.
///
/// A column with no value passes [`is`] against [`Value::Null`] and
/// [`is_not`] against anything else, and fails every test of order.
///
/// Values compare the same way on every device: text and bytes byte
/// by byte, reals in the total order of IEEE 754.
///
/// [`is`]: Self::is
/// [`is_not`]: Self::is_not
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[must_use]
pub struct Filter(Vec<Test>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Test {
    column: ColumnId,
    passes: Passes,
    value: Value,
}

/// What a row's value has to be, next to the test's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Passes {
    Same,
    Different,
    Below,
    AtMost,
    Above,
    AtLeast,
}

impl Filter {
    /// Every row.
    pub fn all() -> Self {
        Self::default()
    }

    /// Rows that hold `value` in `column`.
    pub fn is(self, column: ColumnId, value: impl Into<Value>) -> Self {
        self.and(column, Passes::Same, value.into())
    }

    /// Rows that hold something else in `column`, or nothing.
    pub fn is_not(self, column: ColumnId, value: impl Into<Value>) -> Self {
        self.and(column, Passes::Different, value.into())
    }

    /// Rows that hold less than `value` in `column`.
    pub fn below(self, column: ColumnId, value: impl Into<Value>) -> Self {
        self.and(column, Passes::Below, value.into())
    }

    /// Rows that hold `value` or less in `column`.
    pub fn at_most(self, column: ColumnId, value: impl Into<Value>) -> Self {
        self.and(column, Passes::AtMost, value.into())
    }

    /// Rows that hold more than `value` in `column`.
    pub fn above(self, column: ColumnId, value: impl Into<Value>) -> Self {
        self.and(column, Passes::Above, value.into())
    }

    /// Rows that hold `value` or more in `column`.
    pub fn at_least(self, column: ColumnId, value: impl Into<Value>) -> Self {
        self.and(column, Passes::AtLeast, value.into())
    }

    fn and(mut self, column: ColumnId, passes: Passes, value: Value) -> Self {
        self.0.push(Test {
            column,
            passes,
            value,
        });
        self
    }

    /// Whether every test compares a column with a value it could
    /// hold.
    pub(crate) fn typed(&self) -> bool {
        self.0.iter().all(|test| test.column.takes(&test.value))
    }

    pub(crate) fn picks(&self, row: &Row) -> bool {
        self.0
            .iter()
            .all(|test| test.passed_by(row.get(test.column)))
    }
}

impl Test {
    fn passed_by(&self, held: &Value) -> bool {
        let order = compare(held, &self.value);
        match self.passes {
            Passes::Same => *held == self.value,
            Passes::Different => *held != self.value,
            Passes::Below => order == Some(Ordering::Less),
            Passes::AtMost => matches!(order, Some(Ordering::Less | Ordering::Equal)),
            Passes::Above => order == Some(Ordering::Greater),
            Passes::AtLeast => matches!(order, Some(Ordering::Greater | Ordering::Equal)),
        }
    }
}

/// How two values of one type compare. `None` if either is missing, or
/// they aren't of one type.
fn compare(held: &Value, value: &Value) -> Option<Ordering> {
    match (held, value) {
        (Value::Integer(held), Value::Integer(value)) => Some(held.cmp(value)),
        (Value::Real(held), Value::Real(value)) => Some(held.total_cmp(value)),
        (Value::Text(held), Value::Text(value)) => Some(held.as_bytes().cmp(value.as_bytes())),
        (Value::Boolean(held), Value::Boolean(value)) => Some(held.cmp(value)),
        (Value::Bytes(held), Value::Bytes(value)) => Some(held.cmp(value)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_compare_within_a_type_and_not_across() {
        let less = Some(Ordering::Less);

        assert_eq!(compare(&Value::Integer(-2), &Value::Integer(1)), less);
        assert_eq!(compare(&Value::Real(-0.0), &Value::Real(0.0)), less);
        assert_eq!(compare(&"B".into(), &"a".into()), less);
        assert_eq!(compare(&false.into(), &true.into()), less);
        assert_eq!(compare(&Value::Integer(1), &Value::Real(2.0)), None);
        assert_eq!(compare(&Value::Null, &Value::Integer(1)), None);
    }

    /// Which of 4, 5 and 6 pass a test against 5, and whether a
    /// column with nothing in it does.
    fn passing(passes: Passes) -> ([bool; 3], bool) {
        let column = ColumnId::from_bytes([1; 8]);
        let test = Test {
            column,
            passes,
            value: Value::Integer(5),
        };

        (
            [4, 5, 6].map(|held| test.passed_by(&Value::Integer(held))),
            test.passed_by(&Value::Null),
        )
    }

    #[test]
    fn each_test_passes_what_it_says() {
        assert_eq!(passing(Passes::Same), ([false, true, false], false));
        assert_eq!(passing(Passes::Different), ([true, false, true], true));
        assert_eq!(passing(Passes::Below), ([true, false, false], false));
        assert_eq!(passing(Passes::AtMost), ([true, true, false], false));
        assert_eq!(passing(Passes::Above), ([false, false, true], false));
        assert_eq!(passing(Passes::AtLeast), ([false, true, true], false));
    }
}
