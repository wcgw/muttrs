//! voight in muttrs (wcgw-kit's D17): `:voight`, its statusline segment, the `VoightHeat*` groups (in [`crate::hl`])
//! and `<leader>h`, which shades the Pager's prose as the Heatmap of its Assessment.

use wcgw_kit::keymap::{Mode, Topic};
use wcgw_kit::shell::{Command, Segment, Side};

use super::{Kit, Muttrs};
use crate::assessment::{Assessed, Readiness};

/// What voight is about, in `:help`.
const VOIGHT: Topic = Topic::new(78, "voight");

/// Adds `:voight`, its key and its segment to `kit`.
pub(super) fn install(kit: &mut Kit) {
    kit.add_command(
        Command::new("voight", 2, |muttrs: &mut Muttrs, kit, line| {
            muttrs.voight(kit, line.args)
        })
        .help("voight: heatmap shows or hides the Heatmap; on, off it")
        .complete(|_, _, before, _| {
            if before.is_empty() {
                ["heatmap", "on", "off"].map(str::to_owned).to_vec()
            } else {
                Vec::new()
            }
        }),
    );
    kit.keymap_mut().bind(
        &[Mode::Normal],
        "<leader>h",
        "voight heatmap",
        "voight's Heatmap",
        VOIGHT,
    );
    kit.add_segment(Segment::new(Side::Right, 2, |muttrs: &Muttrs, kit| {
        muttrs.assessment_said(kit)
    }));
}

impl Muttrs {
    /// `:voight`: `heatmap` shows the Heatmap or stops; `on` and `off` show it or not; alone, says how it stands.
    fn voight(&mut self, kit: &mut Kit, args: &str) -> Result<super::Effects, String> {
        if self.assessments.is_none() {
            return Err("voight is off: voight = \"cpu\" in config.toml turns it on".to_owned());
        }
        let heatmap = match args {
            "heatmap" => !self.heatmap,
            "on" => true,
            "off" => false,
            "" => {
                kit.info(
                    self.assessment_said(kit)
                        .unwrap_or_else(|| "voight has nothing to say of this".to_owned()),
                );
                return Ok(Vec::new());
            }
            other => return Err(format!("E475: Invalid argument: {other}")),
        };
        self.heatmap = heatmap;
        let current = kit.current_buffer();
        if self.heatmap
            && self
                .pagers
                .get(&current)
                .is_none_or(|pager| self.heat(pager).is_none())
        {
            kit.info("The Heatmap shows on prose voight has assessed");
        }
        self.dirty.extend(self.pagers.keys().copied());
        Ok(Vec::new())
    }

    /// What the statusline says of voight while a Pager shows: that it's loading, assessing, or its Assessment, with
    /// `Heatmap` while that shows; `None` when there's nothing to say, as for a body with too little prose.
    pub(super) fn assessment_said(&self, kit: &Kit) -> Option<String> {
        let assessments = self.assessments.as_ref()?;
        let prose = self.pagers.get(&kit.current_buffer())?.prose()?;
        match assessments.readiness() {
            Readiness::Loading => return Some("voight loading…".to_owned()),
            Readiness::Failed => return None,
            Readiness::Ready => {}
        }
        match assessments.get(prose) {
            None | Some(Assessed::Assessing) => Some("voight …".to_owned()),
            Some(Assessed::Done(Some(assessment))) => {
                let heatmap = if self.heatmap { " Heatmap" } else { "" };
                Some(format!("voight {:.0}%{heatmap}", assessment.share * 100.0))
            }
            Some(Assessed::Done(None)) => None,
            Some(Assessed::Failed(_)) => Some("voight failed".to_owned()),
        }
    }
}
