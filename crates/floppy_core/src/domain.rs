//! Validated dependency-leaf value types shared across simulation modules.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DomainError {
    InvalidAxis(i8),
    InvalidSpinDirection(i8),
    InvalidPart { slot: usize, value: u8 },
    StatOutOfRange(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Player,
    Opponent,
}

impl Side {
    pub const fn index(self) -> usize {
        match self {
            Self::Player => 0,
            Self::Opponent => 1,
        }
    }

    pub const fn other(self) -> Self {
        match self {
            Self::Player => Self::Opponent,
            Self::Opponent => Self::Player,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Axis(i8);

impl Axis {
    pub fn new(value: i8) -> Result<Self, DomainError> {
        (-1..=1)
            .contains(&value)
            .then_some(Self(value))
            .ok_or(DomainError::InvalidAxis(value))
    }

    pub const fn get(self) -> i8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpinDirection(i8);

impl SpinDirection {
    pub fn new(value: i8) -> Result<Self, DomainError> {
        matches!(value, -1 | 1)
            .then_some(Self(value))
            .ok_or(DomainError::InvalidSpinDirection(value))
    }

    pub const fn get(self) -> i8 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GarageSelection([u8; 5]);

impl GarageSelection {
    pub fn new(parts: [u8; 5]) -> Result<Self, DomainError> {
        for (slot, value) in parts.into_iter().enumerate() {
            if value >= 4 {
                return Err(DomainError::InvalidPart { slot, value });
            }
        }
        Ok(Self(parts))
    }

    pub const fn parts(self) -> [u8; 5] {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_values_are_rejected_at_construction() {
        assert!(Axis::new(2).is_err());
        assert!(SpinDirection::new(0).is_err());
        assert!(GarageSelection::new([0, 1, 2, 3, 4]).is_err());
    }
}
