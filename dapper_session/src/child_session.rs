// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Declarative child-session configuration: types describing whether and how a
//! headless `dapper proxy from-config` session spawns child debug sessions for
//! the adapter's `startDebugging` reverse requests.

use std::path::PathBuf;

use dapper_dap_protocol::requests::StartDebuggingRequestArguments;
use serde::Deserialize;
use serde::Serialize;

use crate::Port;
use crate::config::DebugRequest;
use crate::config::DebugSessionConfig;
use crate::config::SpawnConfig;
use crate::config::StdioSpawnConfig;
use crate::config::TcpSpawnConfig;
#[cfg(unix)]
use crate::config::UdsSpawnConfig;

/// Whether and how this proxy spawns child sessions for the adapter's
/// `startDebugging` reverse requests. Off by default; enabling it lets the
/// adapter spawn local `dapper` processes, so enable only for trusted configs.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionConfig {
    /// Handle `startDebugging` by spawning child sessions. When false, reverse
    /// requests fail closed.
    #[serde(default)]
    pub auto_spawn: bool,
    /// Max concurrent direct children. `0` disables (and un-advertises the
    /// capability); omitted defaults to 16 — never unlimited (fork-bomb safety).
    #[serde(default = "default_max_children")]
    pub max_children: u32,
    /// Descendant generations allowed. `0` disables; `1` allows one (the common
    /// debugpy case); each child carries `max_depth - 1`. Omitted defaults to 1.
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// The declarative rule set mapping a `startDebugging` reverse request to a
    /// child session configuration.
    #[serde(default)]
    pub profile: ChildSessionProfile,
}

fn default_max_children() -> u32 {
    16
}

fn default_max_depth() -> u32 {
    1
}

/// A declarative child-session profile: an ordered list of rules plus an
/// optional message used when no rule applies.
///
/// Deserializes from either an explicit `{ rules, unsupportedMessage }` object
/// or a bundled preset name (e.g. `"debugpy"`/`"lldb-dap"`), which expands to
/// the same shape (see the custom `Deserialize` impl).
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionProfile {
    /// Evaluated in order; first applicable wins, empty list fails closed. A
    /// rule with an empty `when` matches anything, so order any catch-all last
    /// (it also shadows `unsupported_message`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<ChildSessionRule>,
    /// Optional message surfaced when no rule applies (e.g. the lldb-dap
    /// preset's stdio-parent explanation). When absent, a generic
    /// "no matching child-session rule" message is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_message: Option<String>,
}

/// The `unsupportedMessage` carried by the `lldb-dap` preset. With a stdio
/// parent the preset's only rule is action-incompatible, so the reverse request
/// fails closed with this declarative message rather than a Rust branch.
pub const LLDB_DAP_STDIO_UNSUPPORTED_MESSAGE: &str = "startDebugging unsupported for lldb-dap profile with stdio parent backend; lldb-dap session handoff requires a reusable tcp/uds server endpoint";

impl ChildSessionProfile {
    /// The bundled `debugpy` preset: a connect-back rule. debugpy with
    /// `subProcess: true` emits `attach` with `configuration.connect.{host,port}`,
    /// so the child attaches there (no `parentBackend` constraint).
    pub fn debugpy_preset() -> Self {
        ChildSessionProfile {
            rules: vec![ChildSessionRule {
                when: RuleCondition {
                    request: Some("attach".to_string()),
                    exists: vec![
                        "configuration.connect.host".to_string(),
                        "configuration.connect.port".to_string(),
                    ],
                    parent_backend: vec![],
                },
                child_backend: ChildBackendTemplate::Tcp {
                    host: "${configuration.connect.host}".to_string(),
                    port: PortTemplate::Template("${configuration.connect.port}".to_string()),
                },
                debug_request: DebugRequestTemplate {
                    request: "${request}".to_string(),
                    arguments: serde_json::Value::String("${configuration}".to_string()),
                },
            }],
            unsupported_message: None,
        }
    }

    /// The bundled `lldb-dap` preset: reuse the parent's reusable tcp/uds server
    /// for the handoff (lldb-dap resolves the handed-off target IDs in the same
    /// adapter process). A stdio parent has no applicable rule, so the
    /// declarative `unsupportedMessage` explains the fail-closed case.
    pub fn lldb_dap_preset() -> Self {
        ChildSessionProfile {
            rules: vec![ChildSessionRule {
                when: RuleCondition {
                    request: None,
                    exists: vec![],
                    parent_backend: vec![BackendKind::Tcp, BackendKind::Uds],
                },
                child_backend: ChildBackendTemplate::ParentBackend,
                debug_request: DebugRequestTemplate {
                    request: "${request}".to_string(),
                    arguments: serde_json::Value::String("${configuration}".to_string()),
                },
            }],
            unsupported_message: Some(LLDB_DAP_STDIO_UNSUPPORTED_MESSAGE.to_string()),
        }
    }

    /// Expand a named preset to its explicit rule set, or `None` for an unknown
    /// name. Used by the `Deserialize` impl so a config can name a bundled preset
    /// instead of inlining rules.
    fn from_preset_name(name: &str) -> Option<Self> {
        match name {
            "debugpy" => Some(Self::debugpy_preset()),
            "lldb-dap" => Some(Self::lldb_dap_preset()),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for ChildSessionProfile {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Accept a preset name (bare string) or an explicit { rules,
        // unsupportedMessage } object; a preset expands to the same shape, so the
        // rest of the engine stays preset-agnostic. Branch on a parsed `Value`
        // (not `#[serde(untagged)]`) so a malformed object yields serde's precise
        // field error, not "did not match any variant".
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Explicit {
            #[serde(default)]
            rules: Vec<ChildSessionRule>,
            #[serde(default)]
            unsupported_message: Option<String>,
        }

        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::String(name) => {
                ChildSessionProfile::from_preset_name(&name).ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "unknown child-session profile preset '{name}'; expected \"debugpy\", \"lldb-dap\", or an explicit {{ rules, unsupportedMessage }} object"
                    ))
                })
            }
            serde_json::Value::Object(_) => {
                let e: Explicit =
                    serde_json::from_value(value).map_err(serde::de::Error::custom)?;
                Ok(ChildSessionProfile {
                    rules: e.rules,
                    unsupported_message: e.unsupported_message,
                })
            }
            other => Err(serde::de::Error::custom(format!(
                "childSessions profile must be a preset name (\"debugpy\"/\"lldb-dap\") or an explicit {{ rules, unsupportedMessage }} object, got {other}"
            ))),
        }
    }
}

/// A single child-session rule: when `when` matches the reverse request and is
/// compatible with the parent backend, the child is built from `child_backend`
/// and `debug_request` (with `${...}` templating resolved against the request).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ChildSessionRule {
    /// When this rule applies. Empty matches any request (still subject to
    /// `child_backend` action-compatibility, checked by the resolver).
    #[serde(default)]
    pub when: RuleCondition,
    /// How to spawn or connect to the child backend.
    pub child_backend: ChildBackendTemplate,
    /// The debug request to send to the child session.
    pub debug_request: DebugRequestTemplate,
}

/// Match conditions for a child-session rule. All present conditions must hold
/// for the rule to apply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct RuleCondition {
    /// If set, the reverse request's `request` field must equal this exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    /// Dotted paths that must all be present in the reverse request, rooted at
    /// the request object — so a path into `configuration` includes that prefix
    /// (e.g. `"configuration.connect.host"`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exists: Vec<String>,
    /// If non-empty, the parent backend's kind must be one of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parent_backend: Vec<BackendKind>,
}

/// The transport kind of a backend, used in rule conditions and
/// action-compatibility checks. All variants exist on every platform so configs
/// parse portably; the Unix-only `uds` parent simply never matches on non-Unix.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Stdio,
    Tcp,
    Uds,
}

/// How to construct the child backend; `${...}` templates are resolved against
/// the reverse request by the resolver.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ChildBackendTemplate {
    /// Connect to a TCP DAP server at the (templated) host/port.
    Tcp { host: String, port: PortTemplate },
    /// Connect to a Unix Domain Socket at the (templated) path.
    Uds { path: String },
    /// Spawn a process and communicate via stdio.
    Stdio {
        cmd: String,
        #[serde(default)]
        args: Vec<String>,
    },
    /// Reuse the parent's spawn config verbatim. Valid only when the parent
    /// backend is `tcp`/`uds` (a reusable server endpoint).
    ParentBackend,
    /// Reuse the parent's stdio `cmd`/`args`. Valid only when the parent backend
    /// is `stdio`.
    InheritParentStdio,
}

/// A port in a `tcp` child backend: either a `${...}` template / numeric string,
/// or a literal port number. The resolver coerces the resolved value to a `Port`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PortTemplate {
    /// A `${...}` template or a numeric string (resolved/coerced by the resolver).
    Template(String),
    /// A literal port number.
    Literal(u16),
}

/// The debug request to send to a child session, with `${...}` templating
/// resolved against the reverse request by the resolver.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DebugRequestTemplate {
    /// The request kind (e.g. `"attach"`) or a template such as `"${request}"`.
    pub request: String,
    /// The request arguments, or a template such as `"${configuration}"`.
    pub arguments: serde_json::Value,
}

/// Error returned when a `startDebugging` reverse request cannot be resolved
/// into a child `DebugSessionConfig`.
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    /// `max_depth` is 0: the permitted number of descendant generations is
    /// exhausted, so no child may be spawned.
    #[error("child-session depth exhausted (max_depth is 0)")]
    DepthExhausted,
    /// No profile rule applied to this reverse request for the current parent
    /// backend. Carries the profile's `unsupported_message` when set, otherwise
    /// a generic explanation.
    #[error("{0}")]
    NoMatchingRule(String),
    /// A `${...}` template referenced a path absent from the reverse request.
    #[error("template path `${{{path}}}` not found in the reverse request")]
    MissingTemplatePath { path: String },
    /// A `${...}` template or literal resolved to a value of the wrong shape
    /// (e.g. a non-numeric port, or non-object debug-request arguments).
    #[error("template `{path}` resolved to an invalid value: {reason}")]
    InvalidTemplateValue { path: String, reason: String },
    /// A rule's `child_backend` action is incompatible with the parent backend
    /// (e.g. `parentBackend` with a stdio parent). Normally filtered out by
    /// `can_resolve_for_parent_backend`; surfaced here defensively.
    #[error("incompatible child backend: {0}")]
    IncompatibleBackend(IncompatibleBackend),
}

/// Why a rule's `child_backend` action can't apply to the current parent backend.
#[derive(Debug, thiserror::Error)]
pub enum IncompatibleBackend {
    #[error("`parentBackend` requires a reusable tcp/uds parent")]
    ParentBackendNeedsReusableParent,
    #[error("`inheritParentStdio` requires a stdio parent")]
    InheritParentStdioNeedsStdioParent,
    #[error("`uds` child backend is not supported on this platform")]
    UdsUnsupportedOnPlatform,
}

/// Resolve a `startDebugging` reverse request into a child `DebugSessionConfig`
/// using the parent's declarative `child_sessions` profile.
///
/// Builds the child spawn config and debug request from the first applicable
/// rule (resolving `${...}` templates), inherits selected parent fields, and
/// decrements `max_depth`. Does not stamp the parent linkage — that's the
/// supervisor/tracker's job (it has no parent `SessionId` here).
///
/// Effectively pure for the common case; a `tcp` child backend whose host is
/// not an IP literal resolves via blocking DNS (`ToSocketAddrs`) here.
pub fn resolve_child_session(
    parent: &DebugSessionConfig,
    args: &StartDebuggingRequestArguments,
) -> Result<DebugSessionConfig, ResolveError> {
    let child_sessions = parent.child_sessions.as_ref().ok_or_else(|| {
        ResolveError::NoMatchingRule("child sessions are not configured".to_string())
    })?;

    // Fail closed (no underflow) when depth is exhausted; the decrement below
    // runs only on the `> 0` path.
    if child_sessions.max_depth == 0 {
        return Err(ResolveError::DepthExhausted);
    }

    // The substitution context is a JSON view of the reverse request, so
    // `${request}` and `${configuration...}` paths resolve against it.
    let context = serde_json::to_value(args).map_err(|e| ResolveError::InvalidTemplateValue {
        path: "<reverse request>".to_string(),
        reason: e.to_string(),
    })?;

    let rule = child_sessions
        .profile
        .rules
        .iter()
        .find(|rule| {
            rule_matches(rule, args, &context)
                && can_resolve_for_parent_backend(rule, &parent.spawn_config)
        })
        .ok_or_else(|| {
            let message = child_sessions
                .profile
                .unsupported_message
                .clone()
                .unwrap_or_else(|| {
                    "no matching child-session rule for this parent backend".to_string()
                });
            ResolveError::NoMatchingRule(message)
        })?;

    let spawn_config =
        build_child_spawn_config(&rule.child_backend, &parent.spawn_config, &context)?;
    let debug_request = build_child_debug_request(&rule.debug_request, &context)?;

    // Carry the child-session config forward with one fewer generation so peer
    // grandchildren self-govern; `max_children` and `profile` are inherited.
    let child_child_sessions = ChildSessionConfig {
        max_depth: child_sessions.max_depth - 1,
        ..child_sessions.clone()
    };

    Ok(DebugSessionConfig {
        spawn_config,
        debug_request: Some(debug_request),
        breakpoints: parent.breakpoints.clone(),
        metadata: parent.metadata.clone(),
        initialize_args: parent.initialize_args.clone(),
        init_timeout_secs: parent.init_timeout_secs,
        install_default_exception_breakpoints: parent.install_default_exception_breakpoints,
        child_sessions: Some(child_child_sessions),
    })
}

/// Whether `rule` can apply to a parent with the given spawn config, based only
/// on static information (no reverse request needed). Checks the
/// `when.parent_backend` constraint AND the `child_backend` action's
/// compatibility with the parent backend kind. Reused by the capability gate so
/// support is advertised only when a rule's action can actually work.
pub fn can_resolve_for_parent_backend(rule: &ChildSessionRule, parent_spawn: &SpawnConfig) -> bool {
    let parent_kind = backend_kind(parent_spawn);

    if !rule.when.parent_backend.is_empty() && !rule.when.parent_backend.contains(&parent_kind) {
        return false;
    }

    match &rule.child_backend {
        // Reusing the parent endpoint requires a reusable server (tcp/uds).
        ChildBackendTemplate::ParentBackend => {
            matches!(parent_kind, BackendKind::Tcp | BackendKind::Uds)
        }
        // Reusing the parent's stdio command requires a stdio parent.
        ChildBackendTemplate::InheritParentStdio => matches!(parent_kind, BackendKind::Stdio),
        // A literal `uds` child backend is only resolvable on Unix (off-Unix
        // `build_uds_spawn_config` always fails).
        ChildBackendTemplate::Uds { .. } => cfg!(unix),
        // Literal tcp/stdio backends stand alone and work for any parent kind.
        ChildBackendTemplate::Tcp { .. } | ChildBackendTemplate::Stdio { .. } => true,
    }
}

fn backend_kind(spawn: &SpawnConfig) -> BackendKind {
    match spawn {
        SpawnConfig::Stdio(_) => BackendKind::Stdio,
        SpawnConfig::Tcp(_) => BackendKind::Tcp,
        #[cfg(unix)]
        SpawnConfig::Uds(_) => BackendKind::Uds,
    }
}

/// Whether `rule`'s request-dependent conditions (`when.request`,
/// `when.exists`) match the reverse request. The static parent-backend
/// conditions are checked separately by `can_resolve_for_parent_backend`.
fn rule_matches(
    rule: &ChildSessionRule,
    args: &StartDebuggingRequestArguments,
    context: &serde_json::Value,
) -> bool {
    if let Some(expected) = &rule.when.request
        && &args.request.to_string() != expected
    {
        return false;
    }
    rule.when
        .exists
        .iter()
        .all(|path| context.pointer(&dotted_to_pointer(path)).is_some())
}

fn build_child_spawn_config(
    backend: &ChildBackendTemplate,
    parent_spawn: &SpawnConfig,
    context: &serde_json::Value,
) -> Result<SpawnConfig, ResolveError> {
    match backend {
        ChildBackendTemplate::Tcp { host, port } => {
            let host = resolve_string_template(host, context)?;
            let port = resolve_port(port, context)?;
            let addr = crate::resolve_socket_addr(&host, port.get()).map_err(|e| {
                ResolveError::InvalidTemplateValue {
                    path: format!("{host}:{port}"),
                    reason: e.to_string(),
                }
            })?;
            Ok(SpawnConfig::Tcp(TcpSpawnConfig {
                // The child connects to an already-running server; nothing to spawn.
                cmd: PathBuf::new(),
                args: Vec::new(),
                addr,
            }))
        }
        ChildBackendTemplate::Uds { path } => {
            let path = resolve_string_template(path, context)?;
            build_uds_spawn_config(path)
        }
        ChildBackendTemplate::Stdio { cmd, args } => {
            let cmd = resolve_string_template(cmd, context)?;
            let args = args
                .iter()
                .map(|arg| resolve_string_template(arg, context))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(SpawnConfig::Stdio(StdioSpawnConfig {
                cmd,
                args,
                // Headless child adapter: own session so Ctrl+C reaches dapper,
                // not the debuggee (matches the `from_file` forcing).
                new_session: true,
            }))
        }
        ChildBackendTemplate::ParentBackend => match parent_spawn {
            SpawnConfig::Stdio(_) => Err(ResolveError::IncompatibleBackend(
                IncompatibleBackend::ParentBackendNeedsReusableParent,
            )),
            // tcp/uds are reusable server endpoints — clone the parent verbatim.
            reusable => Ok(reusable.clone()),
        },
        ChildBackendTemplate::InheritParentStdio => match parent_spawn {
            SpawnConfig::Stdio(stdio) => Ok(SpawnConfig::Stdio(stdio.clone())),
            _ => Err(ResolveError::IncompatibleBackend(
                IncompatibleBackend::InheritParentStdioNeedsStdioParent,
            )),
        },
    }
}

#[cfg(unix)]
fn build_uds_spawn_config(path: String) -> Result<SpawnConfig, ResolveError> {
    Ok(SpawnConfig::Uds(UdsSpawnConfig {
        path: PathBuf::from(path),
    }))
}

#[cfg(not(unix))]
fn build_uds_spawn_config(_path: String) -> Result<SpawnConfig, ResolveError> {
    Err(ResolveError::IncompatibleBackend(
        IncompatibleBackend::UdsUnsupportedOnPlatform,
    ))
}

fn build_child_debug_request(
    template: &DebugRequestTemplate,
    context: &serde_json::Value,
) -> Result<DebugRequest, ResolveError> {
    let request = resolve_string_template(&template.request, context)?;
    let arguments = resolve_templates(&template.arguments, context)?;

    let mut object = match arguments {
        serde_json::Value::Object(map) => map,
        other => {
            return Err(ResolveError::InvalidTemplateValue {
                path: "debugRequest.arguments".to_string(),
                reason: format!("expected a JSON object, got {other}"),
            });
        }
    };
    // `DebugRequest` is internally tagged by `request`; inject the resolved kind.
    object.insert("request".to_string(), serde_json::Value::String(request));

    serde_json::from_value(serde_json::Value::Object(object)).map_err(|e| {
        ResolveError::InvalidTemplateValue {
            path: "debugRequest".to_string(),
            reason: e.to_string(),
        }
    })
}

/// Recursively resolve `${...}` templates in `template` against `context`. A
/// string node exactly equal to `"${path}"` is replaced by the (type-preserved)
/// value at `path`; other scalars pass through; objects/arrays recurse.
fn resolve_templates(
    template: &serde_json::Value,
    context: &serde_json::Value,
) -> Result<serde_json::Value, ResolveError> {
    match template {
        serde_json::Value::String(s) => match parse_template(s) {
            Some(path) => context
                .pointer(&dotted_to_pointer(path))
                .cloned()
                .ok_or_else(|| ResolveError::MissingTemplatePath {
                    path: path.to_string(),
                }),
            None => Ok(template.clone()),
        },
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| resolve_templates(item, context))
            .collect::<Result<Vec<_>, _>>()
            .map(serde_json::Value::Array),
        serde_json::Value::Object(map) => map
            .iter()
            .map(|(key, value)| Ok((key.clone(), resolve_templates(value, context)?)))
            .collect::<Result<serde_json::Map<_, _>, _>>()
            .map(serde_json::Value::Object),
        scalar => Ok(scalar.clone()),
    }
}

/// Resolve a string field that may be a `"${path}"` template into a `String`. A
/// non-template string is returned verbatim; a template must resolve to a JSON
/// string.
fn resolve_string_template(
    value: &str,
    context: &serde_json::Value,
) -> Result<String, ResolveError> {
    let Some(path) = parse_template(value) else {
        return Ok(value.to_string());
    };
    match context.pointer(&dotted_to_pointer(path)) {
        Some(serde_json::Value::String(resolved)) => Ok(resolved.clone()),
        Some(other) => Err(ResolveError::InvalidTemplateValue {
            path: path.to_string(),
            reason: format!("expected a string, got {other}"),
        }),
        None => Err(ResolveError::MissingTemplatePath {
            path: path.to_string(),
        }),
    }
}

/// Resolve a `PortTemplate` into a `Port`, coercing a JSON number or numeric
/// string (whether a rule literal or substituted from the reverse request).
fn resolve_port(port: &PortTemplate, context: &serde_json::Value) -> Result<Port, ResolveError> {
    match port {
        // Validate the same as a coerced port (e.g. reject 0), one gate for both.
        PortTemplate::Literal(p) => coerce_port(&serde_json::Value::from(*p), "port"),
        PortTemplate::Template(template) => {
            let (value, source) = match parse_template(template) {
                Some(path) => {
                    let resolved = context
                        .pointer(&dotted_to_pointer(path))
                        .cloned()
                        .ok_or_else(|| ResolveError::MissingTemplatePath {
                            path: path.to_string(),
                        })?;
                    (resolved, path.to_string())
                }
                // A literal numeric string in the rule, e.g. "5678".
                None => (
                    serde_json::Value::String(template.clone()),
                    template.clone(),
                ),
            };
            coerce_port(&value, &source)
        }
    }
}

fn coerce_port(value: &serde_json::Value, source: &str) -> Result<Port, ResolveError> {
    let port = match value {
        serde_json::Value::Number(n) => n.as_u64().and_then(|n| u16::try_from(n).ok()),
        serde_json::Value::String(s) => s.parse::<u16>().ok(),
        _ => None,
    };
    // Port 0 is never a valid connect target.
    port.and_then(Port::try_new)
        .ok_or_else(|| ResolveError::InvalidTemplateValue {
            path: source.to_string(),
            reason: format!("expected a port number (1..=65535), got {value}"),
        })
}

/// Inner path of an exact `"${path}"` template, else `None` (a literal).
/// Partial (`"x-${y}"`) and empty (`"${}"`) templates count as literals —
/// substitution is whole-node only, which preserves value types.
fn parse_template(s: &str) -> Option<&str> {
    let inner = s.strip_prefix("${")?.strip_suffix('}')?;
    (!inner.is_empty()).then_some(inner)
}

/// Translate a dotted path (`configuration.connect.host`) to an RFC 6901 JSON
/// pointer (`/configuration/connect/host`), escaping `~` and `/` in segments.
fn dotted_to_pointer(path: &str) -> String {
    path.split('.')
        .map(|segment| format!("/{}", segment.replace('~', "~0").replace('/', "~1")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::DebugSessionConfig;

    #[test]
    fn test_child_sessions_debugpy_rule_roundtrip() {
        // The debugpy-style connect-back rule: attach to the host/port the
        // adapter hands back in `configuration.connect`.
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "python", "args": ["-m", "debugpy.adapter"] },
            "childSessions": {
                "autoSpawn": true,
                "profile": {
                    "rules": [
                        {
                            "when": {
                                "request": "attach",
                                "exists": ["configuration.connect.host", "configuration.connect.port"]
                            },
                            "childBackend": {
                                "type": "tcp",
                                "host": "${configuration.connect.host}",
                                "port": "${configuration.connect.port}"
                            },
                            "debugRequest": {
                                "request": "${request}",
                                "arguments": "${configuration}"
                            }
                        }
                    ]
                }
            }
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        let child = config
            .child_sessions
            .clone()
            .expect("childSessions present");
        assert!(child.auto_spawn);
        // Omitted limits fall back to the finite defaults, never unlimited.
        assert_eq!(child.max_children, 16, "maxChildren default should be 16");
        assert_eq!(child.max_depth, 1, "maxDepth default should be 1");
        assert_eq!(child.profile.rules.len(), 1);

        let rule = &child.profile.rules[0];
        assert_eq!(rule.when.request.as_deref(), Some("attach"));
        assert_eq!(
            rule.when.exists,
            vec![
                "configuration.connect.host".to_string(),
                "configuration.connect.port".to_string()
            ]
        );
        assert!(rule.when.parent_backend.is_empty());
        assert_eq!(
            rule.child_backend,
            ChildBackendTemplate::Tcp {
                host: "${configuration.connect.host}".to_string(),
                port: PortTemplate::Template("${configuration.connect.port}".to_string()),
            }
        );
        assert_eq!(rule.debug_request.request, "${request}");
        assert_eq!(
            rule.debug_request.arguments,
            serde_json::json!("${configuration}")
        );

        // Round-trips back to an equivalent childSessions value.
        let serialized = serde_json::to_string(&config).unwrap();
        let reparsed: DebugSessionConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed.child_sessions, config.child_sessions);
    }

    #[test]
    fn test_child_sessions_lldb_dap_rule_roundtrip_with_unsupported_message() {
        // The lldb-dap-style handoff rule: reuse the parent's reusable server
        // backend. Also carries a declarative unsupportedMessage.
        let json = r#"{
            "spawnConfig": { "type": "tcp", "cmd": "/usr/bin/lldb-dap", "addr": "127.0.0.1:12345" },
            "childSessions": {
                "autoSpawn": true,
                "maxChildren": 4,
                "maxDepth": 2,
                "profile": {
                    "rules": [
                        {
                            "when": { "parentBackend": ["tcp", "uds"] },
                            "childBackend": { "type": "parentBackend" },
                            "debugRequest": {
                                "request": "${request}",
                                "arguments": "${configuration}"
                            }
                        }
                    ],
                    "unsupportedMessage": "startDebugging unsupported for lldb-dap profile with stdio parent backend; lldb-dap session handoff requires a reusable tcp/uds server endpoint"
                }
            }
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        let child = config
            .child_sessions
            .clone()
            .expect("childSessions present");
        assert!(child.auto_spawn);
        assert_eq!(child.max_children, 4);
        assert_eq!(child.max_depth, 2);

        let rule = &child.profile.rules[0];
        assert!(rule.when.request.is_none());
        assert!(rule.when.exists.is_empty());
        assert_eq!(
            rule.when.parent_backend,
            vec![BackendKind::Tcp, BackendKind::Uds]
        );
        assert_eq!(rule.child_backend, ChildBackendTemplate::ParentBackend);
        assert_eq!(
            child.profile.unsupported_message.as_deref(),
            Some(
                "startDebugging unsupported for lldb-dap profile with stdio parent backend; lldb-dap session handoff requires a reusable tcp/uds server endpoint"
            )
        );

        let serialized = serde_json::to_string(&config).unwrap();
        let reparsed: DebugSessionConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(reparsed.child_sessions, config.child_sessions);
    }

    #[test]
    fn test_child_sessions_limits_default_and_explicit() {
        // Omitted limits -> finite defaults.
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
            "childSessions": { "autoSpawn": true, "profile": { "rules": [] } }
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        let child = config.child_sessions.unwrap();
        assert_eq!(child.max_children, 16);
        assert_eq!(child.max_depth, 1);
        assert!(child.profile.rules.is_empty());
        assert!(child.profile.unsupported_message.is_none());

        // Explicit maxDepth: 0 (disables spawning) round-trips as 0.
        let json = r#"{
            "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
            "childSessions": { "autoSpawn": false, "maxChildren": 0, "maxDepth": 0, "profile": { "rules": [] } }
        }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        let child = config.child_sessions.unwrap();
        assert!(!child.auto_spawn);
        assert_eq!(child.max_children, 0);
        assert_eq!(child.max_depth, 0);
    }

    #[test]
    fn test_child_sessions_absent_by_default() {
        let json = r#"{ "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" } }"#;
        let config: DebugSessionConfig = serde_json::from_str(json).unwrap();
        assert!(config.child_sessions.is_none());
        // Absent child_sessions is omitted from serialization.
        let serialized = serde_json::to_string(&config).unwrap();
        assert!(!serialized.contains("childSessions"));
    }

    #[test]
    fn test_child_backend_stdio_and_port_literal_roundtrip() {
        // Literal stdio child backend with explicit args.
        let json = r#"{
            "when": {},
            "childBackend": { "type": "stdio", "cmd": "python", "args": ["-m", "debugpy.adapter"] },
            "debugRequest": { "request": "launch", "arguments": { "program": "child.py" } }
        }"#;
        let rule: ChildSessionRule = serde_json::from_str(json).unwrap();
        assert_eq!(
            rule.child_backend,
            ChildBackendTemplate::Stdio {
                cmd: "python".to_string(),
                args: vec!["-m".to_string(), "debugpy.adapter".to_string()],
            }
        );

        // A literal numeric port deserializes into PortTemplate::Literal, while a
        // string port stays a Template for the resolver to substitute/coerce.
        let json = r#"{ "type": "tcp", "host": "127.0.0.1", "port": 5678 }"#;
        let backend: ChildBackendTemplate = serde_json::from_str(json).unwrap();
        assert_eq!(
            backend,
            ChildBackendTemplate::Tcp {
                host: "127.0.0.1".to_string(),
                port: PortTemplate::Literal(5678),
            }
        );

        let json = r#"{ "type": "tcp", "host": "127.0.0.1", "port": "5678" }"#;
        let backend: ChildBackendTemplate = serde_json::from_str(json).unwrap();
        assert_eq!(
            backend,
            ChildBackendTemplate::Tcp {
                host: "127.0.0.1".to_string(),
                port: PortTemplate::Template("5678".to_string()),
            }
        );
    }

    // ----- resolver tests -----

    /// A debugpy-style parent: stdio adapter, connect-back rule keyed on
    /// `request == "attach"` and `configuration.connect.{host,port}`.
    fn debugpy_parent() -> DebugSessionConfig {
        serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "python", "args": ["-m", "debugpy.adapter"] },
                "breakpoints": [{ "type": "function", "name": "main" }],
                "initTimeoutSecs": 123,
                "metadata": { "sessionId": "parent-1" },
                "childSessions": {
                    "autoSpawn": true,
                    "maxDepth": 2,
                    "maxChildren": 4,
                    "profile": {
                        "rules": [{
                            "when": {
                                "request": "attach",
                                "exists": ["configuration.connect.host", "configuration.connect.port"]
                            },
                            "childBackend": {
                                "type": "tcp",
                                "host": "${configuration.connect.host}",
                                "port": "${configuration.connect.port}"
                            },
                            "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                        }]
                    }
                }
            }"#,
        )
        .unwrap()
    }

    fn start_debugging_args(value: serde_json::Value) -> StartDebuggingRequestArguments {
        serde_json::from_value(value).expect("valid startDebugging arguments")
    }

    #[test]
    fn test_resolve_debugpy_connect_back() {
        let parent = debugpy_parent();
        let args = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "connect": { "host": "127.0.0.1", "port": 5678 }, "name": "child" }
        }));

        let child = resolve_child_session(&parent, &args).expect("resolves");

        // tcp child backend resolved from configuration.connect.{host,port}.
        match &child.spawn_config {
            SpawnConfig::Tcp(tcp) => {
                assert_eq!(tcp.addr, "127.0.0.1:5678".parse().unwrap());
                assert_eq!(tcp.cmd, PathBuf::new(), "connect-only child has no cmd");
            }
            other => panic!("expected tcp spawn config, got {other:?}"),
        }

        // debug request: attach carrying the whole configuration as arguments.
        match &child.debug_request {
            Some(DebugRequest::Attach(attach)) => {
                assert!(attach.extra.contains_key("connect"));
                assert_eq!(attach.extra.get("name").unwrap(), "child");
                // `request` is the enum tag, not duplicated into the arguments.
                assert!(!attach.extra.contains_key("request"));
            }
            other => panic!("expected attach debug request, got {other:?}"),
        }

        // Inherited parent fields.
        assert_eq!(child.breakpoints, parent.breakpoints);
        assert_eq!(child.init_timeout_secs, Some(123));
        assert_eq!(child.metadata.get("sessionId").unwrap(), "parent-1");

        // child_sessions carried forward with max_depth decremented (2 -> 1),
        // max_children unchanged.
        let child_cs = child.child_sessions.expect("child carries child_sessions");
        assert_eq!(child_cs.max_depth, 1, "max_depth must decrement by one");
        assert_eq!(child_cs.max_children, 4, "max_children inherited unchanged");
        assert!(child_cs.auto_spawn);
    }

    #[test]
    fn test_resolve_numeric_string_port_coercion() {
        let parent = debugpy_parent();
        let args = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "connect": { "host": "127.0.0.1", "port": "5678" } }
        }));
        let child = resolve_child_session(&parent, &args).expect("resolves");
        match &child.spawn_config {
            SpawnConfig::Tcp(tcp) => assert_eq!(tcp.addr.port(), 5678),
            other => panic!("expected tcp, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_lldb_dap_parent_backend_tcp() {
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "tcp", "cmd": "/usr/bin/lldb-dap", "addr": "127.0.0.1:12345" },
                "childSessions": {
                    "autoSpawn": true,
                    "profile": {
                        "rules": [{
                            "when": { "parentBackend": ["tcp", "uds"] },
                            "childBackend": { "type": "parentBackend" },
                            "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                        }]
                    }
                }
            }"#,
        )
        .unwrap();
        let args = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "session": { "debuggerId": 1, "targetId": 2 } }
        }));

        let child = resolve_child_session(&parent, &args).expect("resolves");
        // The child reuses the parent's reusable tcp endpoint verbatim.
        match &child.spawn_config {
            SpawnConfig::Tcp(tcp) => assert_eq!(tcp.addr, "127.0.0.1:12345".parse().unwrap()),
            other => panic!("expected tcp, got {other:?}"),
        }
        match &child.debug_request {
            Some(DebugRequest::Attach(attach)) => assert!(attach.extra.contains_key("session")),
            other => panic!("expected attach, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_lldb_dap_stdio_parent_fails_closed_with_message() {
        // parentBackend action requires a tcp/uds parent; a stdio parent has no
        // applicable rule, so the declarative unsupportedMessage is surfaced.
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
                "childSessions": {
                    "autoSpawn": true,
                    "profile": {
                        "rules": [{
                            "when": { "parentBackend": ["tcp", "uds"] },
                            "childBackend": { "type": "parentBackend" },
                            "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                        }],
                        "unsupportedMessage": "lldb-dap needs a reusable server"
                    }
                }
            }"#,
        )
        .unwrap();
        let args =
            start_debugging_args(serde_json::json!({ "request": "attach", "configuration": {} }));
        match resolve_child_session(&parent, &args).unwrap_err() {
            ResolveError::NoMatchingRule(msg) => {
                assert_eq!(msg, "lldb-dap needs a reusable server")
            }
            other => panic!("expected NoMatchingRule with custom message, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_inherit_parent_stdio() {
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "myadapter", "args": ["--foo"] },
                "childSessions": {
                    "autoSpawn": true,
                    "profile": {
                        "rules": [{
                            "when": {},
                            "childBackend": { "type": "inheritParentStdio" },
                            "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                        }]
                    }
                }
            }"#,
        )
        .unwrap();
        let args = start_debugging_args(serde_json::json!({
            "request": "launch",
            "configuration": { "program": "x" }
        }));
        let child = resolve_child_session(&parent, &args).expect("resolves");
        match &child.spawn_config {
            SpawnConfig::Stdio(stdio) => {
                assert_eq!(stdio.cmd, "myadapter");
                assert_eq!(stdio.args, vec!["--foo".to_string()]);
            }
            other => panic!("expected stdio, got {other:?}"),
        }
        match &child.debug_request {
            Some(DebugRequest::Launch(launch)) => {
                assert_eq!(launch.extra.get("program").unwrap(), "x")
            }
            other => panic!("expected launch, got {other:?}"),
        }
    }

    #[test]
    fn test_resolve_action_incompatible_with_parent_no_match() {
        // inheritParentStdio with a tcp parent: the only rule's action is
        // incompatible, so no rule applies.
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "tcp", "cmd": "/x", "addr": "127.0.0.1:1" },
                "childSessions": {
                    "autoSpawn": true,
                    "profile": {
                        "rules": [{
                            "when": {},
                            "childBackend": { "type": "inheritParentStdio" },
                            "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                        }]
                    }
                }
            }"#,
        )
        .unwrap();
        let args =
            start_debugging_args(serde_json::json!({ "request": "launch", "configuration": {} }));
        assert!(matches!(
            resolve_child_session(&parent, &args),
            Err(ResolveError::NoMatchingRule(_))
        ));
    }

    #[test]
    fn test_resolve_depth_exhausted_no_underflow() {
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "python" },
                "childSessions": { "autoSpawn": true, "maxDepth": 0, "profile": { "rules": [] } }
            }"#,
        )
        .unwrap();
        let args =
            start_debugging_args(serde_json::json!({ "request": "attach", "configuration": {} }));
        assert!(matches!(
            resolve_child_session(&parent, &args),
            Err(ResolveError::DepthExhausted)
        ));
    }

    #[test]
    fn test_resolve_no_child_sessions() {
        let parent: DebugSessionConfig =
            serde_json::from_str(r#"{ "spawnConfig": { "type": "stdio", "cmd": "python" } }"#)
                .unwrap();
        let args =
            start_debugging_args(serde_json::json!({ "request": "attach", "configuration": {} }));
        assert!(matches!(
            resolve_child_session(&parent, &args),
            Err(ResolveError::NoMatchingRule(_))
        ));
    }

    #[test]
    fn test_resolve_no_matching_rule_generic_and_missing_exists() {
        let parent = debugpy_parent();

        // Wrong request kind: the rule requires attach.
        let launch = start_debugging_args(serde_json::json!({
            "request": "launch",
            "configuration": { "connect": { "host": "127.0.0.1", "port": 5678 } }
        }));
        match resolve_child_session(&parent, &launch).unwrap_err() {
            ResolveError::NoMatchingRule(msg) => {
                assert!(msg.contains("no matching child-session rule"), "got: {msg}")
            }
            other => panic!("expected generic NoMatchingRule, got {other:?}"),
        }

        // Missing required exists path (configuration.connect.port).
        let missing = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "connect": { "host": "127.0.0.1" } }
        }));
        assert!(matches!(
            resolve_child_session(&parent, &missing),
            Err(ResolveError::NoMatchingRule(_))
        ));
    }

    #[test]
    fn test_resolve_invalid_port_values() {
        let parent = debugpy_parent();

        // Non-numeric port string.
        let non_numeric = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "connect": { "host": "127.0.0.1", "port": "not-a-port" } }
        }));
        assert!(matches!(
            resolve_child_session(&parent, &non_numeric),
            Err(ResolveError::InvalidTemplateValue { .. })
        ));

        // Out-of-range port number (> u16::MAX).
        let out_of_range = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "connect": { "host": "127.0.0.1", "port": 70000 } }
        }));
        assert!(matches!(
            resolve_child_session(&parent, &out_of_range),
            Err(ResolveError::InvalidTemplateValue { .. })
        ));

        // Port 0 is not a valid connect target.
        let port_zero = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "connect": { "host": "127.0.0.1", "port": 0 } }
        }));
        assert!(matches!(
            resolve_child_session(&parent, &port_zero),
            Err(ResolveError::InvalidTemplateValue { .. })
        ));
    }

    #[test]
    fn test_resolve_literal_zero_port_rejected() {
        // A literal `port: 0` in the rule (PortTemplate::Literal) is rejected the
        // same as a coerced 0 — it bypassed coerce_port before.
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "x" },
                "childSessions": {
                    "autoSpawn": true,
                    "profile": {
                        "rules": [{
                            "when": {},
                            "childBackend": { "type": "tcp", "host": "127.0.0.1", "port": 0 },
                            "debugRequest": { "request": "${request}", "arguments": "${configuration}" }
                        }]
                    }
                }
            }"#,
        )
        .unwrap();
        let args =
            start_debugging_args(serde_json::json!({ "request": "launch", "configuration": {} }));
        assert!(matches!(
            resolve_child_session(&parent, &args),
            Err(ResolveError::InvalidTemplateValue { .. })
        ));
    }

    #[test]
    fn test_debugpy_preset_expands_to_explicit_rules() {
        // A bare preset name deserializes to the same explicit rule set as the
        // constructor — the preset is purely a deserialization-time convenience.
        let profile: ChildSessionProfile = serde_json::from_str("\"debugpy\"").unwrap();
        assert_eq!(profile, ChildSessionProfile::debugpy_preset());

        assert_eq!(profile.rules.len(), 1);
        let rule = &profile.rules[0];
        assert_eq!(rule.when.request.as_deref(), Some("attach"));
        assert_eq!(
            rule.when.exists,
            vec![
                "configuration.connect.host".to_string(),
                "configuration.connect.port".to_string()
            ]
        );
        // No parentBackend constraint — applies regardless of the parent backend.
        assert!(rule.when.parent_backend.is_empty());
        assert_eq!(
            rule.child_backend,
            ChildBackendTemplate::Tcp {
                host: "${configuration.connect.host}".to_string(),
                port: PortTemplate::Template("${configuration.connect.port}".to_string()),
            }
        );
        assert!(profile.unsupported_message.is_none());
    }

    #[test]
    fn test_lldb_dap_preset_expands_to_explicit_rules() {
        let profile: ChildSessionProfile = serde_json::from_str("\"lldb-dap\"").unwrap();
        assert_eq!(profile, ChildSessionProfile::lldb_dap_preset());

        assert_eq!(profile.rules.len(), 1);
        let rule = &profile.rules[0];
        assert!(rule.when.request.is_none());
        assert!(rule.when.exists.is_empty());
        assert_eq!(
            rule.when.parent_backend,
            vec![BackendKind::Tcp, BackendKind::Uds]
        );
        assert_eq!(rule.child_backend, ChildBackendTemplate::ParentBackend);
        // The exact stdio-parent failure string is data, not a Rust branch.
        assert_eq!(
            profile.unsupported_message.as_deref(),
            Some(LLDB_DAP_STDIO_UNSUPPORTED_MESSAGE)
        );
    }

    #[test]
    fn test_unknown_preset_name_errors() {
        let err = serde_json::from_str::<ChildSessionProfile>("\"bogus\"").unwrap_err();
        assert!(
            err.to_string()
                .contains("unknown child-session profile preset"),
            "error should name the unknown preset, got: {err}"
        );
    }

    #[test]
    fn test_malformed_explicit_profile_surfaces_field_error() {
        // A malformed explicit object surfaces serde's precise type error
        // rather than a generic untagged "did not match any variant" — this is
        // why the Deserialize impl branches on a Value instead of an untagged
        // enum.
        let err = serde_json::from_str::<ChildSessionProfile>(r#"{ "rules": "not-an-array" }"#)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            !msg.contains("did not match any variant"),
            "should not be the generic untagged-enum error, got: {msg}"
        );
        assert!(
            msg.contains("invalid type") || msg.contains("sequence"),
            "error should describe the malformed field shape, got: {msg}"
        );
    }

    #[test]
    fn test_profile_wrong_json_type_errors() {
        // Neither a preset name nor an object -> a clear, actionable error.
        let err = serde_json::from_str::<ChildSessionProfile>("42").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("preset name") || msg.contains("explicit"),
            "error should explain the accepted forms, got: {msg}"
        );
    }

    #[test]
    fn test_explicit_profile_object_still_deserializes() {
        // Backward compatibility: an explicit { rules, unsupportedMessage }
        // object continues to deserialize after presets were added.
        let profile: ChildSessionProfile =
            serde_json::from_str(r#"{ "rules": [], "unsupportedMessage": "custom message" }"#)
                .unwrap();
        assert!(profile.rules.is_empty());
        assert_eq!(
            profile.unsupported_message.as_deref(),
            Some("custom message")
        );
    }

    #[test]
    fn test_named_preset_in_full_config_resolves() {
        // A config can name the preset inline; it resolves end-to-end exactly
        // like the equivalent explicit rule.
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "python", "args": ["-m", "debugpy.adapter"] },
                "childSessions": { "autoSpawn": true, "maxDepth": 1, "maxChildren": 4, "profile": "debugpy" }
            }"#,
        )
        .unwrap();
        assert_eq!(
            parent.child_sessions.as_ref().unwrap().profile,
            ChildSessionProfile::debugpy_preset()
        );

        let args = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "connect": { "host": "127.0.0.1", "port": 5678 } }
        }));
        let child = resolve_child_session(&parent, &args).expect("debugpy preset resolves");
        match &child.spawn_config {
            SpawnConfig::Tcp(tcp) => assert_eq!(tcp.addr, "127.0.0.1:5678".parse().unwrap()),
            other => panic!("expected tcp spawn config, got {other:?}"),
        }
    }

    #[test]
    fn test_lldb_dap_preset_stdio_parent_fails_closed() {
        // lldb-dap handoff needs a reusable tcp/uds server; with a stdio parent
        // the preset's only rule is action-incompatible, so resolution fails
        // closed carrying the declarative unsupportedMessage.
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "stdio", "cmd": "lldb-dap" },
                "childSessions": { "autoSpawn": true, "maxDepth": 1, "maxChildren": 4, "profile": "lldb-dap" }
            }"#,
        )
        .unwrap();

        // The rule is statically incompatible with a stdio parent, so the
        // capability gate would not advertise support for it either.
        let rule = &parent.child_sessions.as_ref().unwrap().profile.rules[0];
        assert!(!can_resolve_for_parent_backend(rule, &parent.spawn_config));

        let args = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "session": { "debuggerId": 1, "targetId": 2 } }
        }));
        match resolve_child_session(&parent, &args) {
            Err(ResolveError::NoMatchingRule(msg)) => {
                assert_eq!(msg, LLDB_DAP_STDIO_UNSUPPORTED_MESSAGE);
            }
            other => panic!("expected NoMatchingRule with the declarative message, got {other:?}"),
        }
    }

    #[test]
    fn test_lldb_dap_preset_tcp_parent_resolves() {
        // With a reusable tcp parent the preset's parentBackend rule applies and
        // the child reuses the parent's server endpoint verbatim.
        let parent: DebugSessionConfig = serde_json::from_str(
            r#"{
                "spawnConfig": { "type": "tcp", "cmd": "", "addr": "127.0.0.1:9000" },
                "childSessions": { "autoSpawn": true, "maxDepth": 1, "maxChildren": 4, "profile": "lldb-dap" }
            }"#,
        )
        .unwrap();

        let args = start_debugging_args(serde_json::json!({
            "request": "attach",
            "configuration": { "session": { "debuggerId": 1, "targetId": 2 } }
        }));
        let child = resolve_child_session(&parent, &args).expect("lldb-dap tcp parent resolves");
        match &child.spawn_config {
            SpawnConfig::Tcp(tcp) => assert_eq!(tcp.addr, "127.0.0.1:9000".parse().unwrap()),
            other => panic!("expected tcp spawn config inherited from parent, got {other:?}"),
        }
    }
}
