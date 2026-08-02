#![cfg(target_os = "macos")]

use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

use serde::Deserialize;
use thiserror::Error;

const EMBEDDED_POLICY: &str = include_str!("../distribution-signing.json");
const POLICY_FORMAT_VERSION: u32 = 1;

#[derive(Clone, Copy)]
pub(crate) enum DistributionSidecar {
    LlamaServer,
    Litestream,
}

#[derive(Debug, Error)]
pub(crate) enum DistributionSignatureError {
    #[error("the embedded distribution signing policy is invalid")]
    InvalidPolicy(#[source] serde_json::Error),
    #[error("the embedded distribution signing policy is unsafe")]
    UnsafePolicy,
    #[error("the sidecar does not have the authorized Developer ID signature")]
    UnauthorizedSignature,
    #[error("the sidecar code-directory hash is unavailable")]
    MissingCodeDirectoryHash,
    #[error("the sidecar could not be inspected")]
    Inspection(#[source] std::io::Error),
}

#[derive(Debug)]
pub(crate) struct VerifiedDistributionSidecar {
    pub(crate) code_directory_hash: String,
    pub(crate) size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DistributionSigningPolicy {
    format_version: u32,
    application: ApplicationSigningPolicy,
    sidecars: SidecarSigningPolicies,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApplicationSigningPolicy {
    signing_identity: String,
    team_identifier: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarSigningPolicies {
    llama_server: SidecarSigningPolicy,
    litestream: SidecarSigningPolicy,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SidecarSigningPolicy {
    component: String,
    identifier: String,
    bundle_path: String,
}

pub(crate) fn verify_distribution_sidecar(
    path: &Path,
    sidecar: DistributionSidecar,
    expected_component: &str,
    expected_bundle_path: &str,
) -> Result<VerifiedDistributionSidecar, DistributionSignatureError> {
    let policy: DistributionSigningPolicy =
        serde_json::from_str(EMBEDDED_POLICY).map_err(DistributionSignatureError::InvalidPolicy)?;
    let sidecar_policy = match sidecar {
        DistributionSidecar::LlamaServer => &policy.sidecars.llama_server,
        DistributionSidecar::Litestream => &policy.sidecars.litestream,
    };
    validate_policy(
        &policy,
        sidecar_policy,
        expected_component,
        expected_bundle_path,
    )?;

    let requirement = code_requirement(&policy, sidecar_policy);
    let status = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict", &format!("-R={requirement}")])
        .arg(path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(DistributionSignatureError::Inspection)?;
    if !status.success() {
        return Err(DistributionSignatureError::UnauthorizedSignature);
    }

    let architecture = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        _ => return Err(DistributionSignatureError::UnsafePolicy),
    };
    let output = Command::new("/usr/bin/codesign")
        .args(["--display", "--verbose=4", "--arch", architecture])
        .arg(path)
        .output()
        .map_err(DistributionSignatureError::Inspection)?;
    if !output.status.success() {
        return Err(DistributionSignatureError::UnauthorizedSignature);
    }
    let description = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    let code_directory_hash = signature_field(&description, "CDHash")
        .filter(|value| is_lower_hex(value, 40))
        .ok_or(DistributionSignatureError::MissingCodeDirectoryHash)?;
    let metadata = fs::symlink_metadata(path).map_err(DistributionSignatureError::Inspection)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(DistributionSignatureError::UnauthorizedSignature);
    }
    Ok(VerifiedDistributionSidecar {
        code_directory_hash,
        size: metadata.len(),
    })
}

fn validate_policy(
    policy: &DistributionSigningPolicy,
    sidecar: &SidecarSigningPolicy,
    expected_component: &str,
    expected_bundle_path: &str,
) -> Result<(), DistributionSignatureError> {
    if policy.format_version != POLICY_FORMAT_VERSION
        || !policy
            .application
            .signing_identity
            .starts_with("Developer ID Application: ")
        || policy.application.team_identifier.len() != 10
        || !policy
            .application
            .team_identifier
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        || sidecar.component != expected_component
        || sidecar.bundle_path != expected_bundle_path
        || !is_bundle_identifier(&sidecar.identifier)
    {
        return Err(DistributionSignatureError::UnsafePolicy);
    }
    Ok(())
}

fn code_requirement(policy: &DistributionSigningPolicy, sidecar: &SidecarSigningPolicy) -> String {
    format!(
        "identifier {} and anchor apple generic and certificate leaf[subject.OU] = {} and certificate leaf[subject.CN] = {}",
        requirement_literal(&sidecar.identifier),
        requirement_literal(&policy.application.team_identifier),
        requirement_literal(&policy.application.signing_identity),
    )
}

fn requirement_literal(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_bundle_identifier(value: &str) -> bool {
    value.split('.').count() >= 2
        && value.split('.').all(|segment| {
            !segment.is_empty()
                && segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

fn signature_field(description: &str, field: &str) -> Option<String> {
    let prefix = format!("{field}=");
    let mut values = description
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .map(str::to_owned);
    let value = values.next()?;
    values.next().is_none().then_some(value)
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_policy_binds_each_sidecar_to_the_kosh_publisher() {
        let policy: DistributionSigningPolicy =
            serde_json::from_str(EMBEDDED_POLICY).expect("distribution policy");
        validate_policy(
            &policy,
            &policy.sidecars.llama_server,
            "llama-server",
            "bin/llama-server",
        )
        .expect("llama-server policy");
        validate_policy(
            &policy,
            &policy.sidecars.litestream,
            "litestream",
            "bin/litestream",
        )
        .expect("Litestream policy");
        assert_eq!(
            code_requirement(&policy, &policy.sidecars.litestream),
            "identifier \"com.rohan.kosh.sidecar.litestream\" and anchor apple generic and certificate leaf[subject.OU] = \"PMZH6ULML8\" and certificate leaf[subject.CN] = \"Developer ID Application: SILO77 LLC (PMZH6ULML8)\"",
        );
    }

    #[test]
    fn policy_rejects_a_different_component() {
        let policy: DistributionSigningPolicy =
            serde_json::from_str(EMBEDDED_POLICY).expect("distribution policy");
        assert!(matches!(
            validate_policy(
                &policy,
                &policy.sidecars.litestream,
                "not-litestream",
                "bin/litestream",
            ),
            Err(DistributionSignatureError::UnsafePolicy)
        ));
    }
}
