//! The D-128 interop policy (Part 2 of #1028, #1224): which CPython-backed
//! import roots a native `pycc build`/`pycc run` -- and `pycc check`, which
//! checks the same contract -- may admit.
//!
//! Three inputs decide it, in precedence order: an explicit CLI policy
//! (`--interop-policy auto|allowlist|deny`, or `--pure`, which is `deny`),
//! then the `[interop]` table of the nearest `pycc.toml` above the entry
//! file, then the `auto` default. `allow` roots come only from a configured
//! `allowlist`, so a CLI switch *to* `allowlist` from any other configured
//! policy has an empty allow set and rejects every CPython-backed root.
//!
//! Resolution is lazy: [`resolve_for_program`] validates the `[interop]`
//! table and picks the policy only when the linked program has at least one
//! `ImportBinding::Foreign`, so a native-only program -- including one that
//! imports only project modules -- is unaffected by a malformed table.
//! `pycc build --ext` never calls it at all (D-244 rule 3).
//!
//! A rejected import is `I0402`, decided per import by [`rejection`], which
//! both `check`'s [`policy_gaps`] and the native build's
//! `crate::foreign_import::classify_for_native_build` call, so the two
//! cannot drift. The policy is evaluated before the embedding gate: `I0403`
//! applies only to a root the policy admits, which is what keeps `deny`'s
//! diagnostic the same on every host and `--target`.

use crate::frontend::FrontendFailure;
use crate::modules::DiscoveredManifest;
use clap::ValueEnum;
use pycc_diag::{Diagnostic, Span};
use pycc_hir::{HirModule, ImportBinding};

/// One of D-128's three interop policies, as `--interop-policy` and
/// `[interop] policy` spell it.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum InteropPolicy {
    Auto,
    Allowlist,
    Deny,
}

impl InteropPolicy {
    fn name(self) -> &'static str {
        match self {
            InteropPolicy::Auto => "auto",
            InteropPolicy::Allowlist => "allowlist",
            InteropPolicy::Deny => "deny",
        }
    }
}

/// The interop flags one `build`/`run`/`check` invocation parsed. clap
/// rejects `--pure` together with `--interop-policy`, so at most one of the
/// two fields is set.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InteropCli {
    pub policy: Option<InteropPolicy>,
    pub pure: bool,
}

impl InteropCli {
    /// The policy the command line selects, with where it came from.
    fn explicit(self) -> Option<(InteropPolicy, PolicySource)> {
        if self.pure {
            return Some((InteropPolicy::Deny, PolicySource::Pure));
        }
        self.policy.map(|policy| (policy, PolicySource::CliFlag))
    }
}

/// Where a policy that can reject an import came from, named in `I0402`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PolicySource {
    Pure,
    CliFlag,
    /// The manifest's display path, with `\` already normalized to `/`.
    Manifest(String),
}

/// A validated `[interop]` table.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ConfiguredPolicy {
    pub(crate) policy: InteropPolicy,
    pub(crate) allow: Vec<String>,
}

/// The policy in force for one program. Only a policy that can reject an
/// import carries a source: `auto` rejects nothing, so it never needs one.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum EffectivePolicy {
    Auto,
    Deny {
        source: PolicySource,
    },
    Allowlist {
        allow: Vec<String>,
        source: PolicySource,
    },
}

/// Validates the raw `[interop]` value a `pycc.toml` carried.
pub(crate) fn validate_table(raw: &toml::Value) -> Result<ConfiguredPolicy, String> {
    let Some(table) = raw.as_table() else {
        return Err(format!(
            "pycc.toml: `interop` must be a table (`[interop]`), not a {}",
            raw.type_str()
        ));
    };
    if let Some(key) = table
        .keys()
        .find(|key| !matches!(key.as_str(), "policy" | "allow"))
    {
        return Err(format!(
            "pycc.toml: unknown key `{key}` in `[interop]` (expected `policy` or `allow`)"
        ));
    }
    let policy = match table.get("policy") {
        None => InteropPolicy::Auto,
        Some(toml::Value::String(value)) => {
            InteropPolicy::from_str(value, false).map_err(|_| {
                format!(
                    "pycc.toml: `[interop] policy` must be `auto`, `allowlist` or `deny`, not `{value}`"
                )
            })?
        }
        Some(other) => {
            return Err(format!(
                "pycc.toml: `[interop] policy` must be a string, not a {}",
                other.type_str()
            ));
        }
    };
    let allow = match table.get("allow") {
        None => Vec::new(),
        Some(toml::Value::Array(entries)) => entries
            .iter()
            .map(validate_root)
            .collect::<Result<Vec<_>, _>>()?,
        Some(other) => {
            return Err(format!(
                "pycc.toml: `[interop] allow` must be an array of strings, not a {}",
                other.type_str()
            ));
        }
    };
    if !allow.is_empty() && policy != InteropPolicy::Allowlist {
        return Err(format!(
            "pycc.toml: `[interop] allow` lists roots, but the policy is `{}`; \
             `allow` applies only with `policy = \"allowlist\"`",
            policy.name()
        ));
    }
    Ok(ConfiguredPolicy { policy, allow })
}

/// One `allow` entry: a direct import root, so non-empty and undotted.
fn validate_root(entry: &toml::Value) -> Result<String, String> {
    match entry {
        toml::Value::String(root) if root.is_empty() => {
            Err("pycc.toml: an `[interop] allow` entry is empty".to_string())
        }
        toml::Value::String(root) if root.contains('.') => Err(format!(
            "pycc.toml: `[interop] allow` entry `{root}` is dotted; \
             `allow` lists direct import roots such as `{}`",
            root.split('.').next().unwrap_or_default()
        )),
        toml::Value::String(root) => Ok(root.clone()),
        other => Err(format!(
            "pycc.toml: `[interop] allow` entries must be strings, not a {}",
            other.type_str()
        )),
    }
}

/// Resolves the effective policy from the discovered manifest and the CLI.
///
/// No filesystem access: `manifest` is the one `crate::modules` already
/// found and parsed. The configured table is validated even when a CLI flag
/// overrides it, so a malformed manifest never passes silently; a validation
/// error is exit 2, naming the manifest's display path.
pub(crate) fn resolve(
    manifest: Option<&DiscoveredManifest>,
    cli: InteropCli,
) -> Result<EffectivePolicy, FrontendFailure> {
    let configured = match manifest {
        Some(manifest) => match &manifest.config.interop {
            Some(raw) => Some((
                validate_table(raw)
                    .map_err(|message| FrontendFailure::input(manifest.display.clone(), message))?,
                PolicySource::Manifest(manifest.display.replace('\\', "/")),
            )),
            None => None,
        },
        None => None,
    };
    let (policy, source, allow) = match (cli.explicit(), configured) {
        (Some((policy, source)), configured) => (
            policy,
            source,
            configured.map(|(table, _)| table.allow).unwrap_or_default(),
        ),
        (None, Some((table, source))) => (table.policy, source, table.allow),
        (None, None) => return Ok(EffectivePolicy::Auto),
    };
    Ok(match policy {
        InteropPolicy::Auto => EffectivePolicy::Auto,
        InteropPolicy::Deny => EffectivePolicy::Deny { source },
        InteropPolicy::Allowlist => EffectivePolicy::Allowlist { allow, source },
    })
}

/// [`resolve`], but only for a program that has a CPython-backed import:
/// every other program gets `auto` without its `[interop]` table being read
/// at all (lazy resolution, #1224).
pub(crate) fn resolve_for_program(
    hir: &HirModule,
    manifest: Option<&DiscoveredManifest>,
    cli: InteropCli,
) -> Result<EffectivePolicy, FrontendFailure> {
    let has_foreign = hir
        .imports
        .iter()
        .any(|binding| matches!(binding, ImportBinding::Foreign { .. }));
    if !has_foreign {
        return Ok(EffectivePolicy::Auto);
    }
    resolve(manifest, cli)
}

fn describe(policy: InteropPolicy, source: &PolicySource) -> String {
    match source {
        PolicySource::Pure => "set by `--pure`".to_string(),
        PolicySource::CliFlag => format!("set by `--interop-policy {}`", policy.name()),
        PolicySource::Manifest(path) => format!("set by `{path}`"),
    }
}

/// The `I0402` for `import {module_path}` under `policy`, or `None` when the
/// policy admits it. The root is the first dot segment, the same rule the
/// embedding gate uses.
pub(crate) fn rejection(
    policy: &EffectivePolicy,
    module_path: &str,
    span: Span,
) -> Option<Diagnostic> {
    let root = module_path.split('.').next().unwrap_or(module_path);
    let message = match policy {
        EffectivePolicy::Auto => return None,
        EffectivePolicy::Deny { source } => format!(
            "`import {module_path}` is a CPython-backed import, which the `deny` interop \
             policy ({}) rejects: it admits no CPython import",
            describe(InteropPolicy::Deny, source)
        ),
        EffectivePolicy::Allowlist { allow, source } => {
            if allow.iter().any(|allowed| allowed == root) {
                return None;
            }
            format!(
                "`import {module_path}` is a CPython-backed import, which the `allowlist` \
                 interop policy ({}) rejects: its root `{root}` is not in `[interop] allow`",
                describe(InteropPolicy::Allowlist, source)
            )
        }
    };
    Some(Diagnostic::error("I0402", message, span))
}

/// `pycc check`'s policy gate: one `I0402` per rejected CPython-backed
/// import, paired with its position in `hir.imports` (the index
/// `src/frontend.rs` maps back to the owning file), in import-table order.
pub(crate) fn policy_gaps(hir: &HirModule, policy: &EffectivePolicy) -> Vec<(usize, Diagnostic)> {
    hir.imports
        .iter()
        .enumerate()
        .filter_map(|(position, binding)| match binding {
            ImportBinding::Foreign {
                module_path, span, ..
            } => rejection(policy, module_path, *span).map(|gap| (position, gap)),
            ImportBinding::Module { .. }
            | ImportBinding::Symbol { .. }
            | ImportBinding::Project { .. } => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
