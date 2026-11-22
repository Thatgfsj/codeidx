//! Match only definitions compatible with evidence retained from the source.

use super::MAX_CANDIDATES;

pub(super) struct Definition {
    pub id: i64,
    pub path: String,
    pub kind: String,
    pub owner: Option<String>,
}

#[derive(Default)]
pub(super) struct Candidates {
    all: Vec<i64>,
    total: usize,
    local: Option<i64>,
    local_count: usize,
}

impl Candidates {
    pub fn select(
        definitions: &[Definition],
        path: &str,
        kind: &str,
        qualifier: Option<&str>,
    ) -> Self {
        let mut result = Self::default();
        for def in definitions {
            let compatible = match kind {
                "bare" => def.kind == "fn" && def.owner.is_none(),
                "owner" => {
                    def.kind == "method"
                        && qualifier.is_some()
                        && def.owner.as_deref() == qualifier
                        && (def.path == path
                            || crate_dir(path)
                                .zip(crate_dir(&def.path))
                                .is_some_and(|(a, b)| a == b)
                            || parent_dir(path) == parent_dir(&def.path))
                }
                "module" => {
                    def.kind == "fn"
                        && def.owner.is_none()
                        && qualifier.is_some_and(|module| module_matches(path, &def.path, module))
                }
                _ => false,
            };
            if !compatible {
                continue;
            }
            result.total += 1;
            if result.all.len() < MAX_CANDIDATES {
                result.all.push(def.id);
            }
            if def.path == path {
                result.local = Some(def.id);
                result.local_count += 1;
            }
        }
        result
    }

    pub fn view(&self) -> (&'static str, &[i64], usize) {
        match self.total {
            0 => ("unresolved", &[], 0),
            1 => ("unique", &self.all, 1),
            _ if self.local_count == 1 => ("same_file", self.local.as_slice(), 1),
            _ => ("ambiguous", &self.all, self.total),
        }
    }
}

/// Resolve the conventional Rust file layout, not imports/aliases or inline
/// modules. Accept only paths related to the caller's source directory; a
/// same-named file in a different crate/package is not enough.
fn module_matches(caller: &str, candidate: &str, qualifier: &str) -> bool {
    let Some((head, tail)) = qualifier.split_once("::") else {
        return match qualifier {
            "self" => caller == candidate,
            "crate" => crate_root(caller, candidate),
            "super" => false,
            _ => {
                conventional_module(module_dir(caller), qualifier, candidate)
                    || conventional_module(parent_dir(caller), qualifier, candidate)
            }
        };
    };
    let Some(base) = (match head {
        "crate" => crate_dir(caller),
        "self" => Some(module_dir(caller)),
        // File layout alone cannot safely infer arbitrary parent modules.
        "super" => None,
        _ => {
            let name = qualifier.replace("::", "/");
            return conventional_module(module_dir(caller), &name, candidate)
                || conventional_module(parent_dir(caller), &name, candidate);
        }
    }) else {
        return false;
    };
    conventional_module(base, &tail.replace("::", "/"), candidate)
}

fn conventional_module(base: &str, module: &str, candidate: &str) -> bool {
    let relative = if base.is_empty() {
        candidate
    } else {
        let Some(relative) = candidate
            .strip_prefix(base)
            .and_then(|path| path.strip_prefix('/'))
        else {
            return false;
        };
        relative
    };
    relative.strip_suffix("/mod.rs") == Some(module) || relative.strip_suffix(".rs") == Some(module)
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(base, _)| base)
}

fn module_dir(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    if matches!(file, "lib.rs" | "main.rs" | "mod.rs") {
        parent_dir(path)
    } else {
        path.strip_suffix(".rs").unwrap_or_else(|| parent_dir(path))
    }
}

fn crate_dir(path: &str) -> Option<&str> {
    let mut offset = 0;
    let mut last = None;
    for component in path.split('/') {
        if component == "src" {
            last = path.get(..offset + component.len());
        }
        offset += component.len() + 1;
    }
    last
}

fn crate_root(caller: &str, candidate: &str) -> bool {
    if caller == candidate && matches!(caller.rsplit('/').next(), Some("lib.rs" | "main.rs")) {
        return true;
    }
    crate_dir(caller).is_some_and(|base| {
        ["lib", "main"]
            .iter()
            .any(|file| conventional_module(base, file, candidate))
    })
}
