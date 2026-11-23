// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use enumset::EnumSet;

use crate::Operation;
use crate::operation::operation_names;

/// The operations a proxy lets callers of its control plane perform. Every
/// allowed operation's [`Operation::requires`] is allowed too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy(EnumSet<Operation>);

impl Policy {
    /// Allows every operation, as a proxy given no policy does.
    pub const UNRESTRICTED: Policy = Policy(EnumSet::all());

    /// The policy allowing `allowed`, refused when it allows an operation
    /// without what that operation requires.
    pub fn try_new(allowed: EnumSet<Operation>) -> Result<Self, IncompletePolicy> {
        match allowed
            .iter()
            .find(|operation| !allowed.is_superset(operation.requires()))
        {
            Some(operation) => Err(IncompletePolicy {
                operation,
                missing: operation.requires() - allowed,
            }),
            None => Ok(Policy(allowed)),
        }
    }

    pub fn allowed(self) -> EnumSet<Operation> {
        self.0
    }

    /// Lets through `call`, which needs `needed`, or says what it lacks.
    pub fn check(self, call: &str, needed: EnumSet<Operation>) -> Result<(), NotPermitted> {
        let missing = needed - self.0;
        if missing.is_empty() {
            return Ok(());
        }
        Err(NotPermitted {
            call: call.to_owned(),
            missing,
            allowed: self.0,
        })
    }
}

/// A call the session's access policy does not allow. Clients built before
/// access policies existed show only this text, so it names what the call
/// needs and what the policy allows.
#[derive(Debug, PartialEq, thiserror::Error)]
#[error(
    "`{call}` needs {}, which this session's access policy does not allow; it allows {}",
    operation_names(.missing),
    operation_names(.allowed)
)]
pub struct NotPermitted {
    /// The control-plane call or DAP command refused.
    pub call: String,
    pub missing: EnumSet<Operation>,
    pub allowed: EnumSet<Operation>,
}

/// A policy that allows an operation without one it requires.
#[derive(Debug, PartialEq, thiserror::Error)]
#[error("allowing {operation} also needs {}", operation_names(.missing))]
pub struct IncompletePolicy {
    pub operation: Operation,
    pub missing: EnumSet<Operation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_may_allow_nothing() {
        assert_eq!(
            Policy::try_new(EnumSet::empty()).map(Policy::allowed),
            Ok(EnumSet::empty())
        );
    }

    #[test]
    fn a_policy_refuses_an_operation_without_what_it_requires() {
        for (allowed, operation, missing) in [
            (
                Operation::Inspect | Operation::Evaluate,
                Operation::Evaluate,
                EnumSet::only(Operation::ReadValues),
            ),
            (
                Operation::Inspect | Operation::ReadMemory,
                Operation::ReadMemory,
                EnumSet::only(Operation::ReadValues),
            ),
            (
                Operation::Inspect | Operation::SetVariable,
                Operation::SetVariable,
                EnumSet::only(Operation::ReadValues),
            ),
            (
                EnumSet::all() - Operation::WriteMemory,
                Operation::Unclassified,
                EnumSet::only(Operation::WriteMemory),
            ),
        ] {
            assert_eq!(
                Policy::try_new(allowed),
                Err(IncompletePolicy { operation, missing }),
                "{allowed:?} should be refused"
            );
        }
    }

    #[test]
    fn an_incomplete_policy_names_what_is_missing() {
        let refusal = Policy::try_new(Operation::Inspect | Operation::ReadMemory)
            .expect_err("read_memory without read_values should be refused");
        assert_eq!(
            refusal.to_string(),
            "allowing read_memory also needs read_values"
        );
    }

    #[test]
    fn the_unrestricted_policy_allows_everything() {
        assert_eq!(Policy::UNRESTRICTED.allowed(), EnumSet::all());
        assert_eq!(Policy::try_new(EnumSet::all()), Ok(Policy::UNRESTRICTED));
    }

    #[test]
    fn a_policy_lets_through_what_it_allows() {
        let minimal = Policy::try_new(Operation::Inspect | Operation::ReadValues)
            .expect("inspect and read_values make a whole policy");
        assert_eq!(
            minimal.check("threads", EnumSet::only(Operation::Inspect)),
            Ok(())
        );
        assert_eq!(
            minimal.check("capabilities", EnumSet::empty()),
            Ok(()),
            "a call that needs nothing is always let through"
        );
        assert_eq!(
            Policy::try_new(EnumSet::empty())
                .expect("an empty policy is whole")
                .check("status", EnumSet::empty()),
            Ok(())
        );
    }

    #[test]
    fn a_policy_refuses_what_it_does_not_allow() {
        let structure = Policy::try_new(EnumSet::only(Operation::Inspect))
            .expect("inspect alone is a whole policy");
        let refusal = structure
            .check("eval_repl", Operation::Evaluate | Operation::Inspect)
            .expect_err("inspect alone should not allow evaluate");
        assert_eq!(
            refusal,
            NotPermitted {
                call: "eval_repl".to_owned(),
                missing: EnumSet::only(Operation::Evaluate),
                allowed: EnumSet::only(Operation::Inspect),
            },
            "only what the policy lacks is missing"
        );
        assert_eq!(
            refusal.to_string(),
            "`eval_repl` needs evaluate, which this session's access policy does not allow; \
             it allows inspect"
        );
        let nothing = Policy::try_new(EnumSet::empty()).expect("an empty policy is whole");
        assert_eq!(
            nothing
                .check("threads", EnumSet::only(Operation::Inspect))
                .expect_err("an empty policy allows nothing")
                .to_string(),
            "`threads` needs inspect, which this session's access policy does not allow; \
             it allows nothing"
        );
    }
}
