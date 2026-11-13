//! What [`diff`](crate::diff) returns beside its [`DiffReport`]: each side's lowering
//! [`Report`], or the [`DiffRejection`] of the sides that did not lower.

use crate::pipeline::{carve_spec, lower_frontend};
use crate::surface::{self, DiffReport};
use crate::{diag, ir, name, Outcome, Report, Spec};

/// The `Err` of a [`diff`](crate::diff) run: at least one spec failed to lower, so the two
/// surfaces are not comparable and no [`DiffReport`] is produced.
///
/// Each side that failed to lower carries its rejection [`Report`], read through
/// [`old_report`](Self::old_report), [`new_report`](Self::new_report), or
/// [`rejections`](Self::rejections) for every failed side in turn. `diff` never panics on a bad
/// spec.
#[derive(Debug, Clone)]
pub struct DiffRejection {
    old: Option<Report>,
    new: Option<Report>,
}

/// Which side of a diff a rejection or a lowering report belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    /// The old (baseline) spec.
    Old,
    /// The new spec.
    New,
}

impl Side {
    /// The stable lowercase label. [`Display`](std::fmt::Display) writes it, and the serialized
    /// form (`#[serde(rename_all = "lowercase")]`) spells each variant the same way.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Old => "old",
            Side::New => "new",
        }
    }
}

impl std::fmt::Display for Side {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl DiffRejection {
    /// The old spec's rejection report, if it failed to lower.
    pub fn old_report(&self) -> Option<&Report> {
        self.old.as_ref()
    }

    /// The new spec's rejection report, if it failed to lower.
    pub fn new_report(&self) -> Option<&Report> {
        self.new.as_ref()
    }

    /// Every side that failed to lower, with its report.
    pub fn rejections(&self) -> impl Iterator<Item = (Side, &Report)> + '_ {
        self.old
            .iter()
            .map(|report| (Side::Old, report))
            .chain(self.new.iter().map(|report| (Side::New, report)))
    }
}

impl std::fmt::Display for DiffRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (side, report) in self.rejections() {
            writeln!(formatter, "{} spec rejected:", side.as_str())?;
            write!(formatter, "{report}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DiffRejection {}

/// The `Ok` companion of a [`diff`](crate::diff) run: both specs lowered, and this is the
/// [`Report`] of each side's lowering, with [`Outcome::Clean`] and the warnings that lowering emitted, up to the
/// spec's `batch_cap`.
///
/// Read through [`old_report`](Self::old_report), [`new_report`](Self::new_report), or
/// [`reports`](Self::reports) for both sides in turn. Serializes as `{"old": …, "new": …}`, keyed
/// by [`Side::as_str`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiffSides {
    old: Report,
    new: Report,
}

impl DiffSides {
    /// The old spec's lowering report.
    pub fn old_report(&self) -> &Report {
        &self.old
    }

    /// The new spec's lowering report.
    pub fn new_report(&self) -> &Report {
        &self.new
    }

    /// Both sides with their reports, old first.
    pub fn reports(&self) -> impl Iterator<Item = (Side, &Report)> + '_ {
        [(Side::Old, &self.old), (Side::New, &self.new)].into_iter()
    }
}

/// Each side whose report carries a diagnostic, headed by the side it came from. Both sides clean
/// render nothing.
impl std::fmt::Display for DiffSides {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (side, report) in self.reports() {
            if !report.diagnostics().is_empty() {
                writeln!(formatter, "{} spec:", side.as_str())?;
                write!(formatter, "{report}")?;
            }
        }
        Ok(())
    }
}

impl serde::Serialize for DiffRejection {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        for (side, report) in self.rejections() {
            map.serialize_entry(side.as_str(), report)?;
        }
        map.end()
    }
}

/// The body of [`diff`](crate::diff), which runs it on the frontend stack.
pub(crate) fn diff_inner(old: &Spec, new: &Spec) -> Result<(DiffReport, DiffSides), DiffRejection> {
    match (lower_side(old), lower_side(new)) {
        (Ok((old_api, old_names, old_report)), Ok((new_api, new_names, new_report))) => {
            let old_surface = surface::build(&old_api, &old_names);
            let new_surface = surface::build(&new_api, &new_names);
            Ok((
                surface::diff(&old_surface, &new_surface),
                DiffSides {
                    old: old_report,
                    new: new_report,
                },
            ))
        }
        // A spec that failed to lower has no surface, so the two are simply not comparable.
        (old_lowered, new_lowered) => Err(DiffRejection {
            old: old_lowered.err(),
            new: new_lowered.err(),
        }),
    }
}

/// Lower one side of a [`diff`](crate::diff): through the carve driver when `spec.carve` asks for
/// it, as [`run_mode`](crate::pipeline::run_mode) does for every other entry point, and the plain
/// frontend otherwise. A side that does not lower returns its rejection [`Report`]; one that
/// lowers returns its [`Outcome::Clean`] report beside the lowered API, so its warnings reach the
/// caller.
fn lower_side(spec: &Spec) -> Result<(ir::Api, name::Names, Report), Report> {
    let carved;
    let spec = if spec.carve.is_on() {
        carved = carve_spec(spec)?;
        &carved
    } else {
        spec
    };
    let mut diags = diag::Diagnostics::new(spec.batch_cap);
    match lower_frontend(spec, &mut diags) {
        Ok((api, names)) => Ok((api, names, Report::new(diags, Outcome::Clean))),
        Err(()) => Err(Report::new(diags, Outcome::Rejected)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `as_str`, `Display`, and the serialized form spell each diff side alike, matching the
    /// lowercase keys a rejection is reported under.
    #[test]
    fn each_side_renders_its_stable_lowercase_name() {
        for (side, name) in [(Side::Old, "old"), (Side::New, "new")] {
            assert_eq!(side.as_str(), name);
            assert_eq!(side.to_string(), name);
            assert_eq!(serde_json::to_value(side).unwrap(), name);
        }
    }
}
