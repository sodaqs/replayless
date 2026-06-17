/// Encode quality preset → (cq, maxrate). Lower cq = higher quality / bigger.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Balanced,
    Smaller,
    Higher,
}

impl Quality {
    pub fn label(self) -> &'static str {
        match self {
            Quality::Balanced => "Balanced",
            Quality::Smaller => "Smaller",
            Quality::Higher => "Higher",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            Quality::Balanced => "q-balanced",
            Quality::Smaller => "q-smaller",
            Quality::Higher => "q-higher",
        }
    }
    pub fn cq(self) -> u32 {
        match self {
            Quality::Balanced => 30,
            Quality::Smaller => 32,
            Quality::Higher => 28,
        }
    }
    pub fn maxrate(self) -> &'static str {
        match self {
            Quality::Balanced => "12M",
            Quality::Smaller => "8M",
            Quality::Higher => "16M",
        }
    }
    /// Rough average compaction ratio for pre-flight estimate.
    pub fn est_ratio(self) -> f64 {
        match self {
            Quality::Balanced => 6.5,
            Quality::Smaller => 9.0,
            Quality::Higher => 5.0,
        }
    }
}

pub mod view;
