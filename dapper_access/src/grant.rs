// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use std::str::FromStr;

use enumset::EnumSet;
use enumset::enum_set;
use serde_with::DeserializeFromStr;
use serde_with::SerializeDisplay;
use strum::Display;
use strum::EnumIter;
use strum::EnumString;
use strum::IntoEnumIterator;
use strum::IntoStaticStr;

use crate::Operation;
use crate::operation::operation_names;

/// A named set of operations, to grant several at once. A released name
/// never changes.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Display,
    EnumString,
    EnumIter,
    IntoStaticStr
)]
#[strum(serialize_all = "snake_case")]
pub enum Preset {
    /// The program's structure, read only and without its values.
    Structure,
    /// The program's structure and values, read only.
    Minimal,
    /// `minimal` plus navigation, breakpoints and stopping the session.
    Standard,
    /// Every operation, unclassified requests included.
    Full,
}

impl Preset {
    pub const fn operations(self) -> EnumSet<Operation> {
        match self {
            Preset::Structure => enum_set!(Operation::Inspect),
            Preset::Minimal => enum_set!(Operation::Inspect | Operation::ReadValues),
            Preset::Standard => enum_set!(
                Operation::Inspect
                    | Operation::ReadValues
                    | Operation::Navigate
                    | Operation::Breakpoints
                    | Operation::Stop
            ),
            Preset::Full => EnumSet::all(),
        }
    }
}

/// An entry of an allow list, by name: a preset or one operation. No preset
/// shares a name with an operation.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    derive_more::Display,
    derive_more::From,
    DeserializeFromStr,
    SerializeDisplay
)]
pub enum Grant {
    #[display("{_0}")]
    Preset(Preset),
    #[display("{_0}")]
    Operation(Operation),
}

impl Grant {
    /// The operations this entry grants: a preset's, or an operation with what
    /// it requires, such as `read_values` for `evaluate`.
    pub fn operations(self) -> EnumSet<Operation> {
        match self {
            Grant::Preset(preset) => preset.operations(),
            Grant::Operation(operation) => operation | operation.requires(),
        }
    }
}

impl FromStr for Grant {
    type Err = UnknownGrant;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        name.parse()
            .map(Grant::Preset)
            .or_else(|_| name.parse().map(Grant::Operation))
            .map_err(|_| UnknownGrant(name.to_owned()))
    }
}

/// A name that is neither a preset nor an operation.
#[derive(Debug, PartialEq, thiserror::Error)]
#[error(
    "`{0}` is neither a preset ({presets}) nor an operation ({operations})",
    presets = preset_names(),
    operations = operation_names(&EnumSet::all())
)]
pub struct UnknownGrant(pub String);

fn preset_names() -> String {
    Preset::iter()
        .map(<&'static str>::from)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_names_never_change() {
        assert_eq!(
            preset_names(),
            "structure, minimal, standard, full",
            "released names are part of dapper's interface"
        );
    }

    #[test]
    fn presets_build_on_each_other() {
        assert_eq!(
            Preset::Minimal.operations(),
            Preset::Structure.operations() | Operation::ReadValues,
            "minimal adds the program's values to its structure"
        );
        assert_eq!(
            Preset::Standard.operations(),
            Preset::Minimal.operations()
                | Operation::Navigate
                | Operation::Breakpoints
                | Operation::Stop,
        );
        assert_eq!(Preset::Full.operations(), EnumSet::all());
    }

    #[test]
    fn no_preset_shares_a_name_with_an_operation() {
        for preset in Preset::iter() {
            assert!(
                preset.to_string().parse::<Operation>().is_err(),
                "`{preset}` should name only a preset"
            );
        }
    }

    #[test]
    fn grants_parse_by_name() {
        assert_eq!("standard".parse(), Ok(Grant::Preset(Preset::Standard)));
        assert_eq!(
            "read_memory".parse(),
            Ok(Grant::Operation(Operation::ReadMemory))
        );
        let unknown = "everything"
            .parse::<Grant>()
            .expect_err("a name that is no preset or operation should not parse");
        assert_eq!(
            unknown.to_string(),
            "`everything` is neither a preset (structure, minimal, standard, full) nor an operation \
             (inspect, read_values, navigate, breakpoints, evaluate, set_variable, read_memory, \
             write_memory, stop, unclassified)"
        );
    }

    #[test]
    fn grants_cross_serde_as_their_names() {
        let grants: Vec<Grant> = serde_json::from_value(serde_json::json!(["minimal", "navigate"]))
            .expect("known names should deserialize");
        assert_eq!(
            grants,
            [
                Grant::Preset(Preset::Minimal),
                Grant::Operation(Operation::Navigate)
            ]
        );
        assert_eq!(
            serde_json::to_value(&grants).expect("grants should serialize"),
            serde_json::json!(["minimal", "navigate"])
        );
        assert!(
            serde_json::from_value::<Vec<Grant>>(serde_json::json!(["minimal", "everything"]))
                .is_err(),
            "an unknown name should fail the whole list, not be skipped"
        );
    }

    #[test]
    fn a_grant_grants_its_operations() {
        assert_eq!(
            Grant::Preset(Preset::Minimal).operations(),
            Preset::Minimal.operations()
        );
        assert_eq!(
            Grant::Operation(Operation::Stop).operations(),
            EnumSet::only(Operation::Stop)
        );
        assert_eq!(
            Grant::Operation(Operation::ReadMemory).operations(),
            Operation::ReadMemory | Operation::ReadValues,
            "an operation comes with what it requires"
        );
        assert_eq!(
            Grant::Operation(Operation::SetVariable).operations(),
            Operation::SetVariable | Operation::ReadValues,
            "set_variable comes with read_values and not evaluate"
        );
    }
}
