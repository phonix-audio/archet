//! Which eight of a list the encoders show.

/// A window of `WIDTH` items over a list of `count`, moved a page at a time.
/// The page is always below `pages()`, and `pages()` is at least one, so an
/// empty list has one blank page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pager {
    count: usize,
    page: usize,
}

impl Pager {
    /// Items on one page: one per encoder.
    pub const WIDTH: usize = 8;

    /// The first page over `count` items.
    pub fn new(count: usize) -> Self {
        Self { count, page: 0 }
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// How many pages the list fills, at least one.
    pub fn pages(&self) -> usize {
        self.count.div_ceil(Self::WIDTH).max(1)
    }

    /// The page shown, 0..pages().
    pub fn page(&self) -> usize {
        self.page
    }

    /// Turn a page forward; false on the last.
    pub fn forward(&mut self) -> bool {
        let can = self.page + 1 < self.pages();
        if can {
            self.page += 1;
        }
        can
    }

    /// Turn a page back; false on the first.
    pub fn back(&mut self) -> bool {
        let can = self.page > 0;
        if can {
            self.page -= 1;
        }
        can
    }

    /// The list changed length. The page stays where it can.
    pub fn set_count(&mut self, count: usize) {
        self.count = count;
        self.page = self.page.min(self.pages() - 1);
    }

    /// The item encoder `index` shows on this page, if the list reaches it.
    pub fn slot(&self, index: u8) -> Option<usize> {
        let i = self.page * Self::WIDTH + index as usize;
        ((index as usize) < Self::WIDTH && i < self.count).then_some(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An empty list is one blank page.
    #[test]
    fn an_empty_list_is_one_blank_page() {
        let p = Pager::new(0);
        assert_eq!(p.pages(), 1);
        assert_eq!(p.page(), 0);
        assert_eq!(p.slot(0), None);
        let mut p = p;
        assert!(!p.forward());
        assert!(!p.back());
    }

    /// Twenty items are three pages, the last one short.
    #[test]
    fn pages_cover_the_list() {
        let mut p = Pager::new(20);
        assert_eq!(p.pages(), 3);
        assert_eq!(p.slot(7), Some(7));
        assert!(p.forward());
        assert!(p.forward());
        assert!(!p.forward(), "past the last page");
        assert_eq!(p.page(), 2);
        assert_eq!(p.slot(3), Some(19));
        assert_eq!(p.slot(4), None);
        assert_eq!(p.slot(8), None, "no ninth encoder");
        assert!(p.back());
        assert_eq!(p.slot(0), Some(8));
    }

    /// A shorter list pulls the page back within reach.
    #[test]
    fn a_shorter_list_pulls_the_page_back() {
        let mut p = Pager::new(20);
        p.forward();
        p.forward();
        p.set_count(5);
        assert_eq!(p.page(), 0);
        assert_eq!(p.pages(), 1);
        p.set_count(16);
        assert_eq!(p.pages(), 2);
        assert_eq!(p.page(), 0);
    }
}
