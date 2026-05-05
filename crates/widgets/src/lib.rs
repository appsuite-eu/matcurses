//! Reusable widget primitives for braille-friendly TUIs.
//!
//! Cursor placement conventions for interactive widgets.
//! The logical cursor is what the braille display / screen reader follows.
//!
//! - Button    `[ Yes ]`     → first letter of the label (`Y`)
//! - Checkbox  `[x] Label`   → between the brackets (on the `x` or the space)
//! - Radio     `(x) Label`   → between the parens (on the `x` or the space)
//! - Link      `Click here`  → first character of the clickable text
//! - TextInput `Label: …`    → end of the entered value
//! - List/Tree current row   → column requested by the row, or position of the
//!                             match if a `search_query` is provided
//!
//! Domain views are assemblies of these primitives: a login dialog = a `Form`
//! with a few `Text` + `Button`s, a room list = a `List` of formatted
//! `ListRow`s, a space tree = a `Tree` of `TreeRow`s. The crate knows
//! nothing about the application domain (no Matrix, no message, no room) —
//! it only renders UI elements.

pub mod button;
pub mod checkbox;
pub mod form;
pub mod link;
pub mod list;
pub mod modal_frame;
pub mod radio;
pub mod status_bar;
pub mod text_input;
pub mod tree;
pub mod wrapped_list;

pub use button::{render_button, Button};
pub use checkbox::{render_checkbox, Checkbox};
pub use form::{focusable_count, render_form, FormField};
pub use link::{render_link, Link};
pub use list::{render_list, ListRow, ListState};
pub use modal_frame::{centered_rect, render_modal_frame, ModalFrame};
pub use radio::{render_radio, Radio};
pub use status_bar::{render_status_bar, StatusBar};
pub use text_input::{render_text_input, TextInput};
pub use tree::{render_tree, TreeRow, TreeRowKind};
pub use wrapped_list::{render_wrapped_list, WrappedLine};
