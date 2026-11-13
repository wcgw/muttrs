//! voight in muttrs (wcgw-kit's D17): `:voight`, its statusline segment, the `VoightHeat*` groups (in [`crate::hl`])
//! and `<leader>h`, which shades the Pager's prose as the Heatmap of its Assessment.

use wcgw_kit::keymap::{Mode, Topic};
use wcgw_kit::shell::{Command, Segment, Side};

use super::{Kit, Muttrs};
use crate::assessment::{Assessed, Assessments, Readiness};

/// What voight is about, in `:help`.
const VOIGHT: Topic = Topic::new(78, "voight");

/// Adds `:voight`, its key and its segment to `kit`.
pub(super) fn install(kit: &mut Kit) {
    kit.add_command(
        Command::new("voight", 2, |muttrs: &mut Muttrs, kit, line| {
            muttrs.voight(kit, line.args)
        })
        .help("voight: alone, what it says; heatmap shows or hides the Heatmap; on and off start and stop it")
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
    kit.add_segment(
        Segment::new(Side::Right, 2, |muttrs: &Muttrs, kit| {
            muttrs.assessment_said(kit)
        })
        .with_group(crate::hl::VOIGHT),
    );
}

impl Muttrs {
    /// `:voight`, as ghd's: `heatmap` shows the Heatmap or stops; `on` and `off` start and stop assessing; alone,
    /// says how it stands.
    fn voight(&mut self, kit: &mut Kit, args: &str) -> Result<super::Effects, String> {
        match args {
            "on" if !self.voight_built => return Err(Self::voight_off(false).to_owned()),
            "on" => {
                if self.assessments.is_none() {
                    let mut assessments = Assessments::default();
                    if let Some(loaded) = self.voight_loaded {
                        assessments.loaded(loaded);
                    }
                    self.assessments = Some(assessments);
                }
                return Ok(Vec::new());
            }
            "off" => {
                self.assessments = None;
                self.heatmap = false;
                self.dirty.extend(self.pagers.keys().copied());
                return Ok(Vec::new());
            }
            _ if self.assessments.is_none() => {
                return Err(Self::voight_off(self.voight_built).to_owned());
            }
            _ => {}
        }
        let heatmap = match args {
            "heatmap" => !self.heatmap,
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

    /// Why there's no voight: stopped by `:voight off`, if it runs, else the configuration's.
    fn voight_off(built: bool) -> &'static str {
        if built {
            "voight is off: :voight on starts it again"
        } else {
            "voight is off: voight = \"cpu\" in config.toml turns it on"
        }
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
