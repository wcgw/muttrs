//! Assessments: voight's estimate of how much of the prose the Pager shows was written by an LLM (see `CONTEXT.md`),
//! as ghd makes them of its Previews. Never evidence of authorship.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

/// Where voight runs its model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Device {
    Cpu,
    /// A Mac's GPU.
    Metal,
    /// An NVIDIA GPU.
    Cuda,
    /// Any other GPU, through wgpu: Vulkan on Linux.
    Wgpu,
}

impl Device {
    /// Whether this muttrs was built to run voight there: CUDA with its `cuda` feature, wgpu with its `wgpu` one, and
    /// Metal with its `metal` one or on Apple silicon, where voight builds with it unless with wgpu, whose fork of
    /// candle doesn't. Whether the machine has one is another matter.
    pub(crate) fn is_built_in(self) -> bool {
        match self {
            Self::Cpu => cfg!(feature = "voight"),
            Self::Metal => cfg!(all(
                feature = "voight",
                any(
                    feature = "metal",
                    all(
                        target_os = "macos",
                        target_arch = "aarch64",
                        not(feature = "wgpu")
                    )
                )
            )),
            Self::Cuda => cfg!(feature = "cuda"),
            Self::Wgpu => cfg!(feature = "wgpu"),
        }
    }
}

/// As the configuration's `voight` names it.
impl fmt::Display for Device {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Cpu => "cpu",
            Self::Metal => "metal",
            Self::Cuda => "cuda",
            Self::Wgpu => "wgpu",
        })
    }
}

/// What an Assessment is of: the prose itself, so that a Message opened again, or the same text elsewhere, isn't
/// assessed again.
pub(crate) type Key = Arc<str>;

/// What voight has to say, as voight's Assessor tells it.
#[derive(Debug, Clone, PartialEq)]
// Built without voight, nothing says anything; the UI still knows what it would hear.
#[cfg_attr(not(feature = "voight"), allow(dead_code))]
pub(crate) enum Event {
    /// The model is loaded. `fallback` says why it runs on the CPU rather than the device asked for, if it does.
    Loaded { fallback: Option<String> },
    /// The model couldn't be loaded: there'll be no Assessments this run.
    Failed(String),
    /// The Assessment of `key`; `None` for a text with too little prose to tell.
    Assessed {
        key: Key,
        assessment: Result<Option<Assessment>, String>,
    },
}

/// An Assessment of a text: the share of it an LLM wrote, overall and in each window voight scored.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Assessment {
    /// Between 0 and 1: the windows' shares, weighted by their words.
    pub(crate) share: f64,
    /// In the order they come in the text.
    pub(crate) windows: Arc<[Window]>,
}

/// A window of prose voight scored on its own: paragraphs of about 100 to 400 words together, or part of a longer
/// one, cut where a sentence ends.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Window {
    /// Where it is in the text voight was given, in bytes.
    pub(crate) range: Range<usize>,
    /// The share of it an LLM wrote, between 0 and 1.
    pub(crate) share: f64,
}

/// voight's shared Assessor, assessing texts under their [`Key`].
#[cfg(feature = "voight")]
pub(crate) type Assessor = voight::assessor::Assessor<Key>;

/// Fetches voight's model and starts its Assessor on `device`, telling `said` what it says. Fetching downloads the
/// model the first time (about 1.7 GB), showing voight's progress on stderr, and blocks meanwhile: call it before
/// the terminal UI takes over the screen.
///
/// # Errors
///
/// Fails if the model can't be fetched.
#[cfg(feature = "voight")]
pub(crate) fn start(
    device: Device,
    said: impl Fn(Event) + Send + 'static,
) -> anyhow::Result<Assessor> {
    use voight::assessor::{Event as Said, Processor};
    let processor = match device {
        Device::Cpu => Processor::Cpu,
        Device::Metal => Processor::Metal,
        Device::Cuda => Processor::Cuda,
        Device::Wgpu => Processor::Wgpu,
    };
    voight::assessor::Assessor::start(voight::Model::Gradient, processor, move |event| {
        said(match event {
            Said::Loaded { fallback } => Event::Loaded { fallback },
            Said::Failed(why) => Event::Failed(why),
            Said::Assessed { key, assessment } => Event::Assessed {
                key,
                assessment: assessment.map(|assessment| {
                    assessment.map(|assessment| Assessment {
                        share: assessment.share,
                        windows: assessment
                            .windows
                            .into_iter()
                            .map(|window| Window {
                                range: window.range,
                                share: window.share,
                            })
                            .collect(),
                    })
                }),
            },
        });
    })
}

/// Where an Assessment stands.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Assessed {
    Assessing,
    /// `None` for a text with too little prose.
    Done(Option<Assessment>),
    /// Why it couldn't be made.
    Failed(String),
}

/// Whether voight's model is ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Readiness {
    Loading,
    Ready,
    /// It couldn't be loaded: there'll be no Assessments this run.
    Failed,
}

/// The Assessments made this run, and whether voight is ready to make more.
#[derive(Debug)]
pub(crate) struct Assessments {
    readiness: Readiness,
    by_key: HashMap<Key, Assessed>,
}

impl Default for Assessments {
    /// None yet, voight's model loading.
    fn default() -> Self {
        Self {
            readiness: Readiness::Loading,
            by_key: HashMap::new(),
        }
    }
}

impl Assessments {
    pub(crate) fn readiness(&self) -> Readiness {
        self.readiness
    }

    /// Notes that voight's model is ready (`true`), or failed to load.
    pub(crate) fn loaded(&mut self, ready: bool) {
        self.readiness = if ready {
            Readiness::Ready
        } else {
            Readiness::Failed
        };
    }

    /// Where the Assessment of `key` stands; `None` if it's never been asked for.
    pub(crate) fn get(&self, key: &str) -> Option<&Assessed> {
        self.by_key.get(key)
    }

    /// Whether voight is ready for another text: its model loaded, and no Assessment under way.
    pub(crate) fn can_assess(&self) -> bool {
        self.readiness == Readiness::Ready
            && !self
                .by_key
                .values()
                .any(|assessed| *assessed == Assessed::Assessing)
    }

    /// Notes that the Assessment of `key` is under way.
    pub(crate) fn assessing(&mut self, key: Key) {
        self.by_key.insert(key, Assessed::Assessing);
    }

    /// Takes in the Assessment of `key`.
    pub(crate) fn assessed(&mut self, key: Key, assessment: Result<Option<Assessment>, String>) {
        let assessed = match assessment {
            Ok(assessment) => Assessed::Done(assessment),
            Err(why) => Assessed::Failed(why),
        };
        self.by_key.insert(key, assessed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(text: &str) -> Key {
        Arc::from(text)
    }

    #[test]
    fn one_assessment_at_a_time_once_loaded() {
        let mut assessments = Assessments::default();
        assert!(!assessments.can_assess(), "still loading");

        assessments.loaded(true);
        assert!(assessments.can_assess());
        assessments.assessing(key("a body"));
        assert!(!assessments.can_assess(), "one under way");
        assert_eq!(assessments.get("a body"), Some(&Assessed::Assessing));

        let made = Assessment {
            share: 0.73,
            windows: Arc::from([Window {
                range: 0..6,
                share: 0.73,
            }]),
        };
        assessments.assessed(key("a body"), Ok(Some(made.clone())));
        assert!(assessments.can_assess());
        assert_eq!(assessments.get("a body"), Some(&Assessed::Done(Some(made))));
        assert_eq!(assessments.get("another"), None);
    }

    #[test]
    fn failed_model_assesses_nothing() {
        let mut assessments = Assessments::default();
        assessments.loaded(false);
        assert_eq!(assessments.readiness(), Readiness::Failed);
        assert!(!assessments.can_assess());
    }
}
