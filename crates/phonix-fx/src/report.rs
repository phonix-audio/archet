//! What `Chain::apply` could not take as written.

use std::fmt;

/// A slot naming a kind the build lacks; the slot passes audio through and
/// keeps its description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Missing {
    pub slot: usize,
    pub kind: String,
}

/// A parameter id the kind does not declare; kept in the description,
/// ignored by the effect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unknown {
    pub slot: usize,
    pub id: String,
}

/// A value brought inside its range, or a variant replaced by the default.
#[derive(Clone, Debug, PartialEq)]
pub struct Clamped {
    pub slot: usize,
    pub id: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ApplyReport {
    pub missing: Vec<Missing>,
    pub unknown: Vec<Unknown>,
    pub clamped: Vec<Clamped>,
}

impl ApplyReport {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.unknown.is_empty() && self.clamped.is_empty()
    }

    /// Whether every kind named was built.
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// Takes `other`'s entries with their slot numbers offset by `base`.
    pub fn absorb(&mut self, base: usize, other: ApplyReport) {
        self.missing.extend(other.missing.into_iter().map(|m| Missing { slot: m.slot + base, ..m }));
        self.unknown.extend(other.unknown.into_iter().map(|u| Unknown { slot: u.slot + base, ..u }));
        self.clamped.extend(other.clamped.into_iter().map(|c| Clamped { slot: c.slot + base, ..c }));
    }
}

impl fmt::Display for ApplyReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_clean() {
            return write!(f, "applied as written");
        }
        for m in &self.missing {
            writeln!(f, "slot {}: no effect of kind {:?} in this build", m.slot, m.kind)?;
        }
        for u in &self.unknown {
            writeln!(f, "slot {}: parameter {:?} is not declared", u.slot, u.id)?;
        }
        for c in &self.clamped {
            writeln!(f, "slot {}: parameter {:?} was clamped", c.slot, c.id)?;
        }
        Ok(())
    }
}
