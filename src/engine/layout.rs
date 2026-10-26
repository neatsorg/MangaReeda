pub fn is_portrait(size: [u32; 2]) -> bool {
    size[1] > size[0]
}

/// How many pages the automatic layout shows starting at a page: two portrait
/// pages side by side, otherwise one. The cover (first page in shifted mode)
/// always stands alone.
pub fn automatic_pair_count(
    first: Option<[u32; 2]>,
    second: Option<[u32; 2]>,
    cover: bool,
) -> usize {
    if !cover && first.is_some_and(is_portrait) && second.is_some_and(is_portrait) {
        2
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PORTRAIT: [u32; 2] = [10, 20];
    const LANDSCAPE: [u32; 2] = [20, 10];

    #[test]
    fn two_portrait_pages_form_a_spread() {
        assert_eq!(
            automatic_pair_count(Some(PORTRAIT), Some(PORTRAIT), false),
            2
        );
    }

    #[test]
    fn landscape_or_missing_pages_stand_alone() {
        assert_eq!(
            automatic_pair_count(Some(LANDSCAPE), Some(PORTRAIT), false),
            1
        );
        assert_eq!(
            automatic_pair_count(Some(PORTRAIT), Some(LANDSCAPE), false),
            1
        );
        assert_eq!(automatic_pair_count(Some(PORTRAIT), None, false), 1);
        assert_eq!(automatic_pair_count(None, Some(PORTRAIT), false), 1);
    }

    #[test]
    fn cover_stands_alone() {
        assert_eq!(
            automatic_pair_count(Some(PORTRAIT), Some(PORTRAIT), true),
            1
        );
    }

    #[test]
    fn square_is_not_portrait() {
        assert!(!is_portrait([10, 10]));
    }
}
