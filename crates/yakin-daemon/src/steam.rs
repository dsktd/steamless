//! Whether the daemon has stepped aside for another process holding the controller.

use yakin_core::device::holders::Holder;

/// What the engine should do after looking at the holders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Step {
    Stay,
    Yield(Holder),
    Reclaim,
}

/// The process the daemon yielded to, if any.
#[derive(Default)]
pub struct Yield(Option<Holder>);
impl Yield {
    pub fn holder(&self) -> Option<&Holder> {
        self.0.as_ref()
    }

    /// Yield to the first foreign holder; reclaim once there is none.
    pub fn step(
        &self,
        holders: &[Holder],
    ) -> Step {
        match (&self.0, holders.first()) {
            (None, Some(holder)) => Step::Yield(holder.clone()),
            (Some(_), None) => Step::Reclaim,
            _ => Step::Stay,
        }
    }

    pub fn yielded(
        &mut self,
        holder: Holder,
    ) {
        self.0 = Some(holder);
    }

    pub fn reclaimed(&mut self) {
        self.0 = None;
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn steam(pid: u32) -> Holder {
        Holder {
            pid,
            comm: "steam".to_string(),
        }
    }

    #[test]
    fn test_yield() {
        let mut state = Yield::default();
        assert_eq!(state.step(&[]), Step::Stay);
        assert_eq!(state.step(&[steam(1), steam(2)]), Step::Yield(steam(1)));
        state.yielded(steam(1));
        assert_eq!(state.holder(), Some(&steam(1)));
        // Still held, by the same process or another
        assert_eq!(state.step(&[steam(1)]), Step::Stay);
        assert_eq!(state.step(&[steam(2)]), Step::Stay);
        assert_eq!(state.step(&[]), Step::Reclaim);
        // A failed reclaim leaves it yielded, so the next tick tries again
        assert_eq!(state.step(&[]), Step::Reclaim);
        state.reclaimed();
        assert_eq!(state.holder(), None);
        assert_eq!(state.step(&[]), Step::Stay);
    }
}
