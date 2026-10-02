//! The packaged catalogue has no Git history. Differing bytes are local edits,
//! never automatically updatable old versions. Avoid upstream's history subprocess
//! (which inherits Git environment/config) while reusing its inventory/diagnostics.

use std::path::Path;

use ai_toolbox_core::{Catalogue, Inventory, Kind, Origin, Survey};

use crate::Result;

pub(super) fn read(root: &Path, catalogue: &Catalogue) -> Result<Survey> {
    let inventory = Inventory::read(root)?;
    let mut current = inventory.clone();
    for item in &mut current.hooks {
        if let Some(def) = catalogue.hook(&item.name) {
            item.hash.clone_from(&def.hash);
        }
    }
    for item in &mut current.skills {
        if let Some(def) = catalogue.skill(&item.name) {
            item.hash.clone_from(&def.hash);
        }
    }
    for item in &mut current.helpers {
        if let Some(def) = catalogue.helper(&item.name) {
            item.hash.clone_from(&def.hash);
        }
    }
    let mut report = ai_toolbox_core::classify(&current, catalogue);
    for item in &mut report.items {
        let original = match item.kind {
            Kind::Hook => inventory.hook(&item.name).map(|i| &i.hash),
            Kind::Skill => inventory.skill(&item.name).map(|i| &i.hash),
            Kind::Helper => inventory.helper(&item.name).map(|i| &i.hash),
            Kind::Server => None,
        };
        if let Some(hash) = original {
            if *hash != item.hash && !matches!(item.origin, Origin::Broken { .. }) {
                item.origin = Origin::Modified;
            }
            item.hash.clone_from(hash);
        }
    }
    let mut findings = ai_toolbox_core::diagnose(root, &inventory, &report, catalogue);
    for finding in &mut findings {
        if finding.code == ai_toolbox_core::doctor::Code::ItemModified {
            finding.what = format!(
                "{} differs from the bundled catalogue; it may be a local edit or an older version",
                finding
                    .path
                    .strip_prefix(root)
                    .unwrap_or(&finding.path)
                    .display()
            );
            finding.advice = Some(
                "Preserved by repair. Only an explicit install preview can propose replacing it."
                    .into(),
            );
        }
    }
    let mut recommendation = ai_toolbox_core::detect::recommend(root);
    recommendation.notes.push("Catalogue history is not consulted: differing installed content is treated as a local modification and is never repaired automatically.".into());
    Ok(Survey {
        state: ai_toolbox_core::state(&inventory, &report, &findings),
        inventory,
        report,
        findings,
        harnesses: ai_toolbox_core::harness::configured(root),
        recommendation,
    })
}
