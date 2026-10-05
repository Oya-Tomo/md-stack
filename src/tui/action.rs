//! Key bindings: what each key press means on each screen.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::Screen;
use super::doc_view::Motion;

/// Something the user asked for, independent of the key that asked for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    /// Repaint the whole screen, in case the terminal shows leftovers.
    Redraw,
    // Session picker
    NextSession,
    PreviousSession,
    OpenSession,
    CloseSessions,
    // Post viewer
    OpenSessions,
    NextPost,
    PreviousPost,
    Move(Motion),
    CopySnippet,
    CopyPost,
    ToggleFollow,
    MovePostList,
}

impl Action {
    pub fn from_key(screen: Screen, key: KeyEvent) -> Option<Self> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let action = match (screen, key.code) {
            (_, KeyCode::Char('c')) if ctrl => Self::Quit,
            (_, KeyCode::Char('l')) if ctrl => Self::Redraw,
            (_, KeyCode::Char('q')) => Self::Quit,

            (Screen::Sessions, KeyCode::Char('j') | KeyCode::Down) => Self::NextSession,
            (Screen::Sessions, KeyCode::Char('k') | KeyCode::Up) => Self::PreviousSession,
            (Screen::Sessions, KeyCode::Enter) => Self::OpenSession,
            (Screen::Sessions, KeyCode::Esc) => Self::CloseSessions,

            (Screen::Posts, KeyCode::Char('d')) if ctrl => Self::Move(Motion::HalfPageDown),
            (Screen::Posts, KeyCode::Char('u')) if ctrl => Self::Move(Motion::HalfPageUp),
            (Screen::Posts, KeyCode::Char('j') | KeyCode::Down) => Self::Move(Motion::LineDown),
            (Screen::Posts, KeyCode::Char('k') | KeyCode::Up) => Self::Move(Motion::LineUp),
            (Screen::Posts, KeyCode::Char('g') | KeyCode::Home) => Self::Move(Motion::Top),
            (Screen::Posts, KeyCode::Char('G') | KeyCode::End) => Self::Move(Motion::Bottom),
            (Screen::Posts, KeyCode::Tab) => Self::Move(Motion::NextSnippet),
            (Screen::Posts, KeyCode::BackTab) => Self::Move(Motion::PreviousSnippet),
            (Screen::Posts, KeyCode::Char('s')) => Self::OpenSessions,
            (Screen::Posts, KeyCode::Char('J' | ']')) => Self::NextPost,
            (Screen::Posts, KeyCode::Char('K' | '[')) => Self::PreviousPost,
            (Screen::Posts, KeyCode::Char('y')) => Self::CopySnippet,
            (Screen::Posts, KeyCode::Char('Y')) => Self::CopyPost,
            (Screen::Posts, KeyCode::Char('f')) => Self::ToggleFollow,
            (Screen::Posts, KeyCode::Char('p')) => Self::MovePostList,
            _ => return None,
        };
        Some(action)
    }
}

/// Key hints shown in the status line.
pub fn key_hints(screen: Screen) -> &'static str {
    match screen {
        Screen::Sessions => "j/k:move  Enter:open  Esc:back  q:quit",
        Screen::Posts => {
            "s:sessions  J/K:post  j/k:scroll  Tab:snippet  y:copy  Y:copy post  \
             f:follow/lock  p:list  q:quit"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn keys_depend_on_the_screen() {
        let j = key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(
            Action::from_key(Screen::Sessions, j),
            Some(Action::NextSession)
        );
        assert_eq!(
            Action::from_key(Screen::Posts, j),
            Some(Action::Move(Motion::LineDown))
        );
        let ctrl_d = key(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert_eq!(
            Action::from_key(Screen::Posts, ctrl_d),
            Some(Action::Move(Motion::HalfPageDown))
        );
        assert_eq!(Action::from_key(Screen::Sessions, ctrl_d), None);
        let ctrl_l = key(KeyCode::Char('l'), KeyModifiers::CONTROL);
        assert_eq!(
            Action::from_key(Screen::Sessions, ctrl_l),
            Some(Action::Redraw)
        );
        assert_eq!(
            Action::from_key(Screen::Posts, ctrl_l),
            Some(Action::Redraw)
        );
    }
}
