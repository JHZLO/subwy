//! IntelliJ's Back and Forward (Cmd+[ and Cmd+]): the places the caret left by jumping, to go back to.
//! A jump is a move to another file, or a move within one that a key or a click asked for (Go to
//! Line, a search result, an outline station); typing and the arrow keys are not.

use std::path::{Path, PathBuf};

/// How many places Back remembers.
const MOST: usize = 50;

/// Where the caret is: a file of the window's folders (relative, as the tabs name it) and its line
/// and column, from zero.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Default)]
pub struct Places {
    back: Vec<Place>,
    forward: Vec<Place>,
    /// Where the caret is, as last told.
    here: Option<Place>,
}

impl Places {
    /// The caret is at `now`. Coming from another file, the place there is left for Back.
    pub fn moved(&mut self, now: Place) {
        if let Some(left) = self.here.take_if(|here| here.path != now.path) {
            self.leave(left);
        }
        self.here = Some(now);
    }

    /// The caret is about to jump within its file: where it is now is left for Back first.
    pub fn jumping(&mut self) {
        if let Some(here) = self.here.clone() {
            self.leave(here);
        }
    }

    /// The place before, leaving where the caret is for Forward. A place whose file is gone (no
    /// `exists`) is passed over.
    pub fn back(&mut self, exists: impl Fn(&Path) -> bool) -> Option<Place> {
        travel(&mut self.back, &mut self.forward, &mut self.here, exists)
    }

    /// The place Back came from, leaving where the caret is for Back again.
    pub fn forward(&mut self, exists: impl Fn(&Path) -> bool) -> Option<Place> {
        travel(&mut self.forward, &mut self.back, &mut self.here, exists)
    }

    /// A new way from here: what Forward had is no longer ahead.
    fn leave(&mut self, place: Place) {
        if self.back.last() != Some(&place) {
            self.back.push(place);
            if self.back.len() > MOST {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }
}

/// Takes the latest place of `from` that still exists and is not where the caret already is; where
/// the caret was goes on `to`. The caret counts as there at once, before its tab reports the move.
fn travel(
    from: &mut Vec<Place>,
    to: &mut Vec<Place>,
    here: &mut Option<Place>,
    exists: impl Fn(&Path) -> bool,
) -> Option<Place> {
    while let Some(place) = from.pop() {
        if !exists(&place.path) || here.as_ref() == Some(&place) {
            continue;
        }
        if let Some(left) = here.replace(place.clone()) {
            to.push(left);
        }
        return Some(place);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str, line: u32) -> Place {
        Place {
            path: PathBuf::from(path),
            line,
            column: 0,
        }
    }

    fn any(_: &Path) -> bool {
        true
    }

    #[test]
    fn back_and_forward_walk_the_files_opened() {
        let mut places = Places::default();
        places.moved(at("a.md", 0));
        places.moved(at("a.md", 7));
        places.moved(at("b.md", 0));
        places.moved(at("c.md", 3));
        assert_eq!(places.back(any), Some(at("b.md", 0)));
        assert_eq!(places.back(any), Some(at("a.md", 7)));
        assert_eq!(places.back(any), None);
        assert_eq!(places.forward(any), Some(at("b.md", 0)));
        assert_eq!(places.forward(any), Some(at("c.md", 3)));
        assert_eq!(places.forward(any), None);
    }

    #[test]
    fn moves_within_a_file_count_only_as_jumps() {
        let mut places = Places::default();
        places.moved(at("a.md", 0));
        places.moved(at("a.md", 1));
        places.jumping();
        places.moved(at("a.md", 40));
        places.moved(at("a.md", 41));
        assert_eq!(places.back(any), Some(at("a.md", 1)));
        assert_eq!(places.back(any), None);
        assert_eq!(places.forward(any), Some(at("a.md", 41)));
    }

    #[test]
    fn arriving_back_is_not_a_new_place() {
        let mut places = Places::default();
        places.moved(at("a.md", 5));
        places.moved(at("b.md", 0));
        assert_eq!(places.back(any), Some(at("a.md", 5)));
        // The tab reports the caret where Back put it.
        places.moved(at("a.md", 5));
        assert_eq!(places.forward(any), Some(at("b.md", 0)));
        places.moved(at("b.md", 0));
        assert_eq!(places.back(any), Some(at("a.md", 5)));
    }

    #[test]
    fn a_new_jump_drops_what_was_ahead() {
        let mut places = Places::default();
        places.moved(at("a.md", 0));
        places.moved(at("b.md", 0));
        places.back(any);
        places.moved(at("a.md", 0));
        places.moved(at("c.md", 0));
        assert_eq!(places.forward(any), None);
        assert_eq!(places.back(any), Some(at("a.md", 0)));
    }

    #[test]
    fn back_passes_over_files_gone() {
        let mut places = Places::default();
        places.moved(at("a.md", 0));
        places.moved(at("gone.md", 0));
        places.moved(at("b.md", 0));
        let exists = |path: &Path| path != Path::new("gone.md");
        assert_eq!(places.back(exists), Some(at("a.md", 0)));
    }

    #[test]
    fn back_remembers_the_latest_places() {
        let mut places = Places::default();
        for n in 0..=MOST as u32 + 5 {
            places.moved(at(&format!("{n}.md"), 0));
        }
        let mut count = 0;
        while places.back(any).is_some() {
            count += 1;
        }
        assert_eq!(count, MOST);
    }
}
