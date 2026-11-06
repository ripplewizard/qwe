use qwe_core::ResourcePolicy;
use qwe_protocol::ProtocolErrorBody;
use serde_json::{Value, json};

fn error(code: &str, message: impl Into<String>) -> ProtocolErrorBody {
    ProtocolErrorBody {
        code: code.into(),
        message: message.into(),
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
pub fn resource_capabilities(linux_scope_available: bool) -> Value {
    if cfg!(target_os = "linux") && linux_scope_available {
        json!({
            "memory_high": "hard",
            "memory_max": "hard",
            "memory_swap_max": "hard",
            "tasks_max": "hard_tasks",
            "cpu_quota": "hard",
            "cpu_seconds": "monitored",
            "limitation": "cgroup v2 limits the Session tree; tasks include processes and threads"
        })
    } else if cfg!(target_os = "macos") {
        json!({
            "memory_high": "unsupported",
            "memory_max": "monitored",
            "memory_swap_max": "unsupported",
            "tasks_max": "monitored",
            "cpu_quota": "unsupported",
            "cpu_seconds": "monitored",
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
            "limitation": "hard Pane-tree limits require a delegated Linux cgroup v2 service"
        })
    }
}

#[must_use]
pub fn matrix(linux_scope_available: bool) -> Value {
    json!({
        "platform": platform(),
        "service_manager": service_manager(),
        "credentials": {
            "fd_inheritance": true,
            "keychain_provider": cfg!(target_os = "macos"),
            "keychain_signed_acl_verified": false
        },
        "resources": resource_capabilities(linux_scope_available),
        "forced_teardown": {
            "session": true,
            "server": true,
            "task_batch": true
        }
    })
}

pub fn validate_resource_policy(
    payload: &Value,
    hard_cgroup: bool,
) -> Result<ResourcePolicy, ProtocolErrorBody> {
    let Some(value) = payload.get("resource_policy") else {
        return Ok(ResourcePolicy::empty());
    };
    if value.is_null() {
        return Ok(ResourcePolicy::empty());
    }
    let policy: ResourcePolicy = serde_json::from_value(value.clone())
        .map_err(|failure| error("invalid_request", failure.to_string()))?;
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
                "invalid_request",
                "cpu_quota_percent must be between 1 and 100000",
            ));
        }
    }
    validate_limit("cpu_seconds", policy.cpu_seconds, 1, 86_400 * 365)?;
    if let (Some(high), Some(maximum)) = (policy.memory_high_bytes, policy.memory_max_bytes) {
        if high > maximum {
            return Err(error(
                "invalid_request",
                "memory_high_bytes must not exceed memory_max_bytes",
            ));
        }
    }
    if !cfg!(target_os = "macos")
        && !hard_cgroup
        && (policy.memory_high_bytes.is_some()
            || policy.memory_max_bytes.is_some()
            || policy.memory_swap_max_bytes.is_some()
            || policy.tasks_max.is_some()
            || policy.cpu_quota_percent.is_some())
    {
        return Err(unsupported(
            "linux_cgroup_v2",
            "hard Pane-tree limits require a delegated cgroup v2 service",
        ));
    }
    if cfg!(target_os = "macos") {
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
    Ok(policy)
}

pub fn merge_resource_policy(
    current: &ResourcePolicy,
    value: &Value,
    hard_cgroup: bool,
) -> Result<ResourcePolicy, ProtocolErrorBody> {
    let object = value
        .as_object()
        .ok_or_else(|| error("invalid_request", "resource_policy must be an object"))?;
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
                    "invalid_request",
                    format!("unknown resource_policy field '{key}'"),
                ));
            }
        };
        merged_object.insert(canonical.into(), value.clone());
    }
    validate_resource_policy(&json!({"resource_policy": merged}), hard_cgroup)
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
                "invalid_request",
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

    #[test]
    fn process_limits_require_a_supported_enforcement_tier() {
        let policy = validate_resource_policy(
            &json!({
                "resource_policy": {"process_limit": 8}
            }),
            true,
        )
        .unwrap();
        assert_eq!(policy.tasks_max, Some(8));
        if !cfg!(target_os = "macos") {
            let failure = validate_resource_policy(
                &json!({
                    "resource_policy": {"process_limit": 8}
                }),
                false,
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
            false,
        )
        .unwrap();
        assert_eq!(policy.cpu_seconds, Some(2));
        assert!(
            validate_resource_policy(
                &json!({
                    "resource_policy": {"cpu_seconds": 0}
                }),
                false
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
            true,
        )
        .unwrap();
        assert_eq!(policy.memory_max_bytes, Some(134_217_728));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_accepts_monitored_memory_and_rejects_hard_high() {
        let policy = validate_resource_policy(
            &json!({
                "resource_policy": {"memory_bytes": 134217728}
            }),
            false,
        )
        .unwrap();
        assert_eq!(policy.memory_max_bytes, Some(134_217_728));
        let failure = validate_resource_policy(
            &json!({
                "resource_policy": {"memory_high_bytes": 134217728}
            }),
            false,
        )
        .unwrap_err();
        assert_eq!(failure.code, "unsupported_capability");
    }
}
