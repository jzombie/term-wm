bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ContextMask: u64 {
        const NONE              = 0;
        const HAS_FOCUS         = 1 << 0;
        const IS_MOBILE         = 1 << 1;
        const IS_FLOATING       = 1 << 2;
        const CAN_SPLIT         = 1 << 3;
        const HAS_PANES         = 1 << 4;
        const MOUSE_CAPTURE     = 1 << 5;
        const CLIPBOARD_ENABLED = 1 << 6;
    }
}

impl Default for ContextMask {
    fn default() -> Self {
        Self::NONE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_none() {
        assert_eq!(ContextMask::default(), ContextMask::NONE);
    }

    #[test]
    fn contains_single_flag() {
        let mask = ContextMask::HAS_FOCUS;
        assert!(mask.contains(ContextMask::HAS_FOCUS));
        assert!(!mask.contains(ContextMask::IS_MOBILE));
    }

    #[test]
    fn combined_flags() {
        let mask = ContextMask::HAS_FOCUS | ContextMask::CAN_SPLIT;
        assert!(mask.contains(ContextMask::HAS_FOCUS));
        assert!(mask.contains(ContextMask::CAN_SPLIT));
        assert!(!mask.contains(ContextMask::IS_MOBILE));
    }

    #[test]
    fn bitmask_filter_check() {
        let required = ContextMask::HAS_FOCUS | ContextMask::CAN_SPLIT;
        let app = ContextMask::HAS_FOCUS | ContextMask::CAN_SPLIT | ContextMask::HAS_PANES;
        assert_eq!((app & required) == required, true);

        let app_partial = ContextMask::HAS_FOCUS | ContextMask::HAS_PANES;
        assert_eq!((app_partial & required) == required, false);
    }
}
