use qwe_core::{ResourcePolicy, ResourceProfile};
use qwe_protocol::ProtocolErrorBody;
use qwe_runtime::ResourceSupport;
use serde_json::{Value, json};
use std::str::FromStr;

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
        details: None,
    }
}

#[must_use]
pub fn platform() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "unsupported"
    }
}

#[must_use]
pub fn service_manager() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        "systemd_user"
    }
    #[cfg(target_os = "macos")]
    {
        "launchd_user"
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        "unsupported"
    }
}

#[must_use]
pub fn resource_capabilities(support: ResourceSupport) -> Value {
    if cfg!(target_os = "linux") {
        let hard_scope = support.hard_scope_available();
        json!({
            "memory_high": if hard_scope { "hard" } else { "unsupported" },
            "memory_max": if hard_scope { "hard" } else { "unsupported" },
            "memory_swap_max": if hard_scope { "hard" } else { "unsupported" },
            "tasks_max": if hard_scope { "hard_tasks" } else { "unsupported" },
            "cpu_quota": if hard_scope { "hard" } else { "unsupported" },
            "cpu_seconds": if hard_scope { "monitored" } else { "unsupported" },
            "cgroup_v2": support.cgroup_v2,
            "systemd_user": support.systemd_user,
            "controllers": {
                "memory": support.memory,
                "cpu": support.cpu,
                "pids": support.pids
            },
            "profiles": profiles_json(),
            "limitation": if support.hard_scope_available() {
                "cgroup v2 limits the Pane process tree; tasks include processes and threads"
            } else {
                "hard limits require cgroup v2, a systemd user manager, and cpu/memory/pids controllers"
            }
        })
    } else if cfg!(target_os = "macos") {
        json!({
            "memory_high": "unsupported",
            "memory_max": "monitored",
            "memory_swap_max": "unsupported",
            "tasks_max": "monitored",
            "cpu_quota": "unsupported",
            "cpu_seconds": "monitored",
            "cgroup_v2": false,
            "systemd_user": false,
            "controllers": {
                "memory": false,
                "cpu": false,
                "pids": false
            },
            "profiles": profiles_json(),
            "limitation": "macOS has no public hard Pane-tree quota; libproc monitoring has bounded polling overshoot"
        })
    } else {
        json!({
            "memory_high": "unsupported",
            "memory_max": "unsupported",
            "memory_swap_max": "unsupported",
            "tasks_max": "unsupported",
            "cpu_quota": "unsupported",
            "cpu_seconds": "monitored",
            "cgroup_v2": false,
            "systemd_user": false,
            "controllers": {
                "memory": false,
                "cpu": false,
                "pids": false
            },
            "profiles": profiles_json(),
            "limitation": "hard Pane-tree limits require a delegated Linux cgroup v2 service"
        })
    }
}

#[must_use]
pub fn profiles_json() -> Value {
    Value::Array(
        ResourceProfile::ALL
            .into_iter()
            .map(|profile| {
                json!({
                    "name": profile.as_str(),
                    "description": profile.description(),
                    "policy": profile.policy()
                })
            })
            .collect(),
    )
}

#[must_use]
pub fn matrix(support: ResourceSupport) -> Value {
    json!({
        "platform": platform(),
        "service_manager": service_manager(),
        "credentials": {
            "fd_inheritance": true,
            "keychain_provider": cfg!(target_os = "macos"),
            "keychain_signed_acl_verified": false
        },
        "resources": resource_capabilities(support),
        "forced_teardown": {
            "session": true,
            "server": true,
            "task_batch": true
        }
    })
}

#[cfg(test)]
pub fn validate_resource_policy(
    payload: &Value,
    support: ResourceSupport,
) -> Result<ResourcePolicy, ProtocolErrorBody> {
    resolve_resource_policy(payload, support).map(|(_, policy)| policy)
}

pub fn resolve_resource_policy(
    payload: &Value,
    support: ResourceSupport,
) -> Result<(Option<ResourceProfile>, ResourcePolicy), ProtocolErrorBody> {
    let profile = match payload.get("resource_profile") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(
            ResourceProfile::from_str(value)
                .map_err(|failure| error("unknown_resource_profile", failure))?,
        ),
        Some(_) => {
            return Err(error(
                "unknown_resource_profile",
                "resource_profile must be a string",
            ));
        }
    };
    let base = profile
        .map(ResourceProfile::policy)
        .unwrap_or_else(ResourcePolicy::empty);
    let policy = match payload.get("resource_policy") {
        None | Some(Value::Null) => {
            validate_policy_limits(&base, support)?;
            base
        }
        Some(value) => merge_resource_policy(&base, value, support)?,
    };
    Ok((profile, policy))
}

fn validate_policy_limits(
    policy: &ResourcePolicy,
    support: ResourceSupport,
) -> Result<(), ProtocolErrorBody> {
    validate_limit("memory_high_bytes", policy.memory_high_bytes, 0, 1 << 50)?;
    validate_limit("memory_max_bytes", policy.memory_max_bytes, 1, 1 << 50)?;
    validate_limit(
        "memory_swap_max_bytes",
        policy.memory_swap_max_bytes,
        0,
        1 << 50,
    )?;
    validate_limit("tasks_max", policy.tasks_max, 1, u64::from(u32::MAX))?;
    if let Some(percent) = policy.cpu_quota_percent {
        if !(1..=100_000).contains(&percent) {
            return Err(error(
                "invalid_resource_policy",
                "cpu_quota_percent must be between 1 and 100000",
            ));
        }
    }
    validate_limit("cpu_seconds", policy.cpu_seconds, 1, 86_400 * 365)?;
    if let (Some(high), Some(maximum)) = (policy.memory_high_bytes, policy.memory_max_bytes) {
        if high > maximum {
            return Err(error(
                "invalid_resource_policy",
                "memory_high_bytes must not exceed memory_max_bytes",
            ));
        }
    }
    if cfg!(target_os = "linux") {
        let hard_limits_requested = policy.memory_high_bytes.is_some()
            || policy.memory_max_bytes.is_some()
            || policy.memory_swap_max_bytes.is_some()
            || policy.tasks_max.is_some()
            || policy.cpu_quota_percent.is_some()
            || policy.cpu_seconds.is_some();
        if (policy.memory_high_bytes.is_some()
            || policy.memory_max_bytes.is_some()
            || policy.memory_swap_max_bytes.is_some())
            && !support.hard_memory()
        {
            return Err(unsupported(
                "linux_cgroup_memory",
                "memory limits require cgroup v2, a systemd user manager, and the memory controller",
            ));
        }
        if policy.tasks_max.is_some() && !support.hard_pids() {
            return Err(unsupported(
                "linux_cgroup_pids",
                "task limits require cgroup v2, a systemd user manager, and the pids controller",
            ));
        }
        if (policy.cpu_quota_percent.is_some() || policy.cpu_seconds.is_some())
            && !support.hard_cpu()
        {
            return Err(unsupported(
                "linux_cgroup_cpu",
                "CPU limits require cgroup v2, a systemd user manager, and the cpu controller",
            ));
        }
        if hard_limits_requested && !support.hard_scope_available() {
            return Err(unsupported(
                "linux_cgroup_v2",
                "hard Pane-tree limits require cgroup v2, a systemd user manager, and cpu/memory/pids controllers",
            ));
        }
    } else if cfg!(target_os = "macos") {
        if policy.memory_high_bytes.is_some() || policy.memory_swap_max_bytes.is_some() {
            return Err(unsupported(
                "macos_memory_high",
                "macOS does not expose hard or throttling Pane-tree memory limits",
            ));
        }
        if policy.cpu_quota_percent.is_some() {
            return Err(unsupported(
                "macos_cpu_quota",
                "macOS does not expose aggregate Pane CPU bandwidth limits",
            ));
        }
    }
    Ok(())
}

pub fn merge_resource_policy(
    current: &ResourcePolicy,
    value: &Value,
    support: ResourceSupport,
) -> Result<ResourcePolicy, ProtocolErrorBody> {
    let object = value
        .as_object()
        .ok_or_else(|| error("invalid_request", "resource_policy must be an object"))?;
    let high_was_explicit = object.contains_key("memory_high_bytes");
    let mut merged = serde_json::to_value(current)
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
    let merged_object = merged
        .as_object_mut()
        .ok_or_else(|| error("internal", "serialized resource policy is not an object"))?;
    for (key, value) in object {
        let canonical = match key.as_str() {
            "memory_high_bytes" => "memory_high_bytes",
            "memory_max_bytes" | "memory_bytes" => "memory_max_bytes",
            "memory_swap_max_bytes" => "memory_swap_max_bytes",
            "tasks_max" | "process_limit" => "tasks_max",
            "cpu_quota_percent" => "cpu_quota_percent",
            "cpu_seconds" => "cpu_seconds",
            _ => {
                return Err(error(
                    "unsupported_resource_field",
                    format!("unsupported resource_policy field '{key}'"),
                ));
            }
        };
        merged_object.insert(canonical.into(), value.clone());
    }
    if !high_was_explicit {
        let high = merged_object
            .get("memory_high_bytes")
            .and_then(Value::as_u64);
        let maximum = merged_object
            .get("memory_max_bytes")
            .and_then(Value::as_u64);
        if let (Some(high), Some(maximum)) = (high, maximum) {
            if high > maximum {
                merged_object.insert("memory_high_bytes".into(), json!(maximum));
            }
        }
    }
    let policy: ResourcePolicy = serde_json::from_value(merged)
        .map_err(|failure| error("invalid_resource_policy", failure.to_string()))?;
    validate_policy_limits(&policy, support)?;
    Ok(policy)
}

fn validate_limit(
    name: &str,
    value: Option<u64>,
    minimum: u64,
    maximum: u64,
) -> Result<(), ProtocolErrorBody> {
    if let Some(value) = value {
        if !(minimum..=maximum).contains(&value) {
            return Err(error(
                "invalid_resource_policy",
                format!("{name} must be between {minimum} and {maximum}"),
            ));
        }
    }
    Ok(())
}

fn unsupported(capability: &str, message: impl Into<String>) -> ProtocolErrorBody {
    error(
        "unsupported_capability",
        format!("{capability}: {}", message.into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete_support() -> ResourceSupport {
        ResourceSupport {
            cgroup_v2: true,
            systemd_user: true,
            memory: true,
            cpu: true,
            pids: true,
        }
    }

    #[test]
    fn process_limits_require_a_supported_enforcement_tier() {
        let policy = validate_resource_policy(
            &json!({
                "resource_policy": {"process_limit": 8}
            }),
            complete_support(),
        )
        .unwrap();
        assert_eq!(policy.tasks_max, Some(8));
        if cfg!(target_os = "linux") {
            let failure = validate_resource_policy(
                &json!({
                    "resource_policy": {"process_limit": 8}
                }),
                ResourceSupport::default(),
            )
            .unwrap_err();
            assert_eq!(failure.code, "unsupported_capability");
        }
    }

    #[test]
    fn validates_cpu_limits() {
        let policy = validate_resource_policy(
            &json!({
                "resource_policy": {"cpu_seconds": 2}
            }),
            complete_support(),
        )
        .unwrap();
        assert_eq!(policy.cpu_seconds, Some(2));
        assert_eq!(
            validate_resource_policy(
                &json!({
                    "resource_policy": {"cpu_seconds": 0}
                }),
                complete_support()
            )
            .unwrap_err()
            .code,
            "invalid_resource_policy"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn lowering_memory_max_clamps_implicit_high() {
        let current = ResourcePolicy {
            memory_high_bytes: Some(128),
            memory_max_bytes: Some(256),
            ..ResourcePolicy::empty()
        };
        let merged = merge_resource_policy(
            &current,
            &json!({"memory_max_bytes": 64}),
            complete_support(),
        )
        .unwrap();
        assert_eq!(merged.memory_high_bytes, Some(64));
        assert_eq!(merged.memory_max_bytes, Some(64));
        assert!(
            merge_resource_policy(
                &current,
                &json!({"memory_high_bytes": 128, "memory_max_bytes": 64}),
                complete_support(),
            )
            .is_err()
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn accepts_linux_memory_limits() {
        let policy = validate_resource_policy(
            &json!({
                "resource_policy": {"memory_bytes": 134217728}
            }),
            complete_support(),
        )
        .unwrap();
        assert_eq!(policy.memory_max_bytes, Some(134_217_728));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn profiles_apply_defaults_and_explicit_overrides() {
        let (profile, policy) = resolve_resource_policy(
            &json!({
                "resource_profile": "tight",
                "resource_policy": {"tasks_max": 32, "memory_max_bytes": 134217728}
            }),
            complete_support(),
        )
        .unwrap();
        assert_eq!(profile, Some(ResourceProfile::Tight));
        assert_eq!(policy.memory_max_bytes, Some(128 * 1024 * 1024));
        assert_eq!(policy.memory_high_bytes, Some(128 * 1024 * 1024));
        assert_eq!(policy.tasks_max, Some(32));
        assert_eq!(policy.cpu_quota_percent, Some(100));
    }

    #[test]
    fn profile_and_field_errors_have_stable_codes() {
        assert_eq!(
            resolve_resource_policy(&json!({"resource_profile": "surprise"}), complete_support())
                .unwrap_err()
                .code,
            "unknown_resource_profile"
        );
        assert_eq!(
            resolve_resource_policy(
                &json!({"resource_policy": {"surprise": 1}}),
                complete_support()
            )
            .unwrap_err()
            .code,
            "unsupported_resource_field"
        );
        assert_eq!(
            resolve_resource_policy(
                &json!({"resource_policy": {"tasks_max": 0}}),
                complete_support()
            )
            .unwrap_err()
            .code,
            "invalid_resource_policy"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_controller_failures_name_the_missing_capability() {
        let failure = resolve_resource_policy(
            &json!({"resource_policy": {"cpu_quota_percent": 50}}),
            ResourceSupport {
                cpu: false,
                ..complete_support()
            },
        )
        .unwrap_err();
        assert_eq!(failure.code, "unsupported_capability");
        assert!(failure.message.contains("linux_cgroup_cpu"));

        let partial_scope = resolve_resource_policy(
            &json!({"resource_policy": {"memory_max_bytes": 1024}}),
            ResourceSupport {
                cpu: false,
                ..complete_support()
            },
        )
        .unwrap_err();
        assert_eq!(partial_scope.code, "unsupported_capability");
        assert!(partial_scope.message.contains("linux_cgroup_v2"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_accepts_monitored_memory_and_rejects_hard_high() {
        let policy = validate_resource_policy(
            &json!({
                "resource_policy": {"memory_bytes": 134217728}
            }),
            ResourceSupport::default(),
        )
        .unwrap();
        assert_eq!(policy.memory_max_bytes, Some(134_217_728));
        let failure = validate_resource_policy(
            &json!({
                "resource_policy": {"memory_high_bytes": 134217728}
            }),
            ResourceSupport::default(),
        )
        .unwrap_err();
        assert_eq!(failure.code, "unsupported_capability");
    }
}
