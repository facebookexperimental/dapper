// Copyright (c) Meta Platforms, Inc. and affiliates.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

use dapper_config::StopConfig;
use dapper_dap_protocol::capabilities::Capabilities;
use dapper_dap_protocol::requests::DisconnectArguments;
use dapper_dap_protocol::requests::RequestCommand;
use dapper_dap_protocol::requests::TerminateArguments;
use dapper_session::RequestType;

/// VS Code runs these types through its launch flow even when the configuration
/// says `attach`. See `getExtensionHostDebugSession` in vscode's `debugUtils.ts`.
const EXTENSION_HOST_TYPES: [&str; 2] = ["extensionhost", "pwa-extensionhost"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionKind {
    Launch,
    Attach,
    Unknown,
}

/// The DAP request that stops a session. Launch sessions terminate the
/// debuggee, attach sessions leave it running, and `terminate_debuggee` /
/// `suspend_debuggee` are only sent to adapters that advertise support.
pub fn stop_request(
    request_type: Option<RequestType>,
    session_type: Option<&str>,
    capabilities: Option<&Capabilities>,
    config: &StopConfig,
) -> RequestCommand {
    let supports_terminate_request = capabilities
        .and_then(|caps| caps.supports_terminate_request)
        .unwrap_or(false);
    let supports_terminate_debuggee = capabilities
        .and_then(|caps| caps.support_terminate_debuggee)
        .unwrap_or(false);
    let supports_suspend_debuggee = capabilities
        .and_then(|caps| caps.support_suspend_debuggee)
        .unwrap_or(false);

    // Last resort: `terminate` is advisory and nothing here waits to escalate.
    // Keyed to the literal launch request, not the reclassification below.
    if request_type == Some(RequestType::Launch)
        && !supports_terminate_debuggee
        && supports_terminate_request
    {
        return RequestCommand::Terminate(Some(TerminateArguments {
            restart: Some(false),
            ..Default::default()
        }));
    }

    let kind = session_kind(request_type, session_type, config);
    let terminate_debuggee = match kind {
        SessionKind::Launch => Some(true),
        SessionKind::Attach => Some(false),
        SessionKind::Unknown => None,
    };

    RequestCommand::Disconnect(Some(DisconnectArguments {
        terminate_debuggee: terminate_debuggee.filter(|_| supports_terminate_debuggee),
        suspend_debuggee: (kind == SessionKind::Attach
            && supports_terminate_debuggee
            && supports_suspend_debuggee)
            .then_some(false),
        ..Default::default()
    }))
}

fn session_kind(
    request_type: Option<RequestType>,
    session_type: Option<&str>,
    config: &StopConfig,
) -> SessionKind {
    if config.treat_extension_host_as_launch && is_extension_host(session_type) {
        return SessionKind::Launch;
    }

    match request_type {
        Some(RequestType::Launch) => SessionKind::Launch,
        Some(RequestType::Attach) => SessionKind::Attach,
        None => SessionKind::Unknown,
    }
}

fn is_extension_host(session_type: Option<&str>) -> bool {
    session_type.is_some_and(|session_type| {
        EXTENSION_HOST_TYPES.contains(&session_type.to_lowercase().as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(
        supports_terminate_request: bool,
        support_terminate_debuggee: bool,
        support_suspend_debuggee: bool,
    ) -> Capabilities {
        Capabilities {
            supports_terminate_request: Some(supports_terminate_request),
            support_terminate_debuggee: Some(support_terminate_debuggee),
            support_suspend_debuggee: Some(support_suspend_debuggee),
            ..Default::default()
        }
    }

    fn disconnect_arguments(command: RequestCommand) -> DisconnectArguments {
        match command {
            RequestCommand::Disconnect(Some(args)) => args,
            other => panic!("expected a disconnect request, got {other:?}"),
        }
    }

    #[test]
    fn launch_prefers_disconnect_over_the_advisory_terminate_request() {
        let args = disconnect_arguments(stop_request(
            Some(RequestType::Launch),
            Some("python"),
            Some(&capabilities(true, true, true)),
            &StopConfig::default(),
        ));

        assert_eq!(
            args.terminate_debuggee,
            Some(true),
            "an adapter that can terminate the debuggee on disconnect should be told to"
        );
    }

    #[test]
    fn launch_falls_back_to_terminate_request_without_terminate_debuggee_support() {
        let command = stop_request(
            Some(RequestType::Launch),
            Some("python"),
            Some(&capabilities(true, false, false)),
            &StopConfig::default(),
        );

        assert_eq!(
            command,
            RequestCommand::Terminate(Some(TerminateArguments {
                restart: Some(false),
                ..Default::default()
            })),
        );
    }

    #[test]
    fn launch_without_either_capability_omits_terminate_debuggee() {
        let args = disconnect_arguments(stop_request(
            Some(RequestType::Launch),
            Some("python"),
            Some(&capabilities(false, false, false)),
            &StopConfig::default(),
        ));

        assert_eq!(
            args.terminate_debuggee, None,
            "an unsupported attribute must be left off so the adapter default applies"
        );
    }

    #[test]
    fn attach_leaves_the_debuggee_running() {
        let args = disconnect_arguments(stop_request(
            Some(RequestType::Attach),
            Some("python"),
            Some(&capabilities(true, true, true)),
            &StopConfig::default(),
        ));

        assert_eq!(
            args.terminate_debuggee,
            Some(false),
            "stopping an attach session must not kill a process dapper did not start"
        );
        assert_eq!(args.suspend_debuggee, Some(false));
    }

    #[test]
    fn suspend_debuggee_is_omitted_when_the_adapter_does_not_support_it() {
        let args = disconnect_arguments(stop_request(
            Some(RequestType::Attach),
            Some("python"),
            Some(&capabilities(true, true, false)),
            &StopConfig::default(),
        ));

        assert_eq!(args.terminate_debuggee, Some(false));
        assert_eq!(args.suspend_debuggee, None);
    }

    #[test]
    fn extension_host_started_with_attach_terminates_the_debuggee() {
        let args = disconnect_arguments(stop_request(
            Some(RequestType::Attach),
            Some("pwa-extensionHost"),
            Some(&capabilities(true, true, true)),
            &StopConfig::default(),
        ));

        assert_eq!(
            args.terminate_debuggee,
            Some(true),
            "VS Code runs extension host sessions through its launch flow"
        );
        assert_eq!(args.suspend_debuggee, None);
    }

    #[test]
    fn extension_host_started_with_attach_does_not_send_a_terminate_request() {
        let command = stop_request(
            Some(RequestType::Attach),
            Some("extensionHost"),
            Some(&capabilities(true, false, false)),
            &StopConfig::default(),
        );

        assert!(
            matches!(command, RequestCommand::Disconnect(_)),
            "VS Code gates the terminate request on the request type alone, got {command:?}"
        );
    }

    #[test]
    fn extension_host_started_with_launch_sends_a_terminate_request() {
        let command = stop_request(
            Some(RequestType::Launch),
            Some("pwa-extensionhost"),
            Some(&capabilities(true, false, false)),
            &StopConfig::default(),
        );

        assert!(matches!(command, RequestCommand::Terminate(_)));
    }

    #[test]
    fn extension_host_follows_its_request_type_when_the_special_case_is_disabled() {
        let args = disconnect_arguments(stop_request(
            Some(RequestType::Attach),
            Some("pwa-extensionHost"),
            Some(&capabilities(true, true, true)),
            &StopConfig {
                treat_extension_host_as_launch: false,
                ..Default::default()
            },
        ));

        assert_eq!(args.terminate_debuggee, Some(false));
        assert_eq!(args.suspend_debuggee, Some(false));
    }

    #[test]
    fn unknown_request_type_leaves_the_decision_to_the_adapter() {
        let args = disconnect_arguments(stop_request(
            None,
            Some("python"),
            Some(&capabilities(true, true, true)),
            &StopConfig::default(),
        ));

        assert_eq!(args.terminate_debuggee, None);
        assert_eq!(args.suspend_debuggee, None);
    }

    #[test]
    fn unknown_request_type_still_terminates_an_extension_host() {
        let args = disconnect_arguments(stop_request(
            None,
            Some("extensionhost"),
            Some(&capabilities(false, true, true)),
            &StopConfig::default(),
        ));

        assert_eq!(args.terminate_debuggee, Some(true));
    }

    #[test]
    fn missing_capabilities_omit_both_attributes() {
        let args = disconnect_arguments(stop_request(
            Some(RequestType::Attach),
            None,
            None,
            &StopConfig::default(),
        ));

        assert_eq!(args.terminate_debuggee, None);
        assert_eq!(args.suspend_debuggee, None);
    }
}
