//! Widgets primitifs réutilisables pour TUI braille-friendly.
//!
//! Conventions de placement du curseur sur les widgets interactifs.
//! Le curseur logique est ce que suit l'afficheur braille / le screen reader.
//!
//! - Bouton    `[ Oui ]`     → première lettre du label (`O`)
//! - Checkbox  `[x] Label`   → entre les crochets (sur le `x` ou l'espace)
//! - Radio     `(x) Label`   → entre les parenthèses (sur le `x` ou l'espace)
//! - Lien      `Cliquez ici` → premier caractère du texte cliquable
//! - TextInput `Label: …`    → fin de la valeur saisie
//! - List/Tree ligne courante → colonne demandée par la row, ou position du
//!                              match si une `search_query` est fournie
//!
//! Les vues métier sont des assemblages de ces primitifs : un dialogue de
//! login = un `Form` avec quelques `Text` + `Button`, une room list = un
//! `List` avec des `ListRow` formatées, un space tree = un `Tree` avec des
//! `TreeRow`. Le crate ne connaît rien du domaine applicatif (pas de
//! Matrix, pas de message, pas de room) — il ne rend que des éléments
//! d'interface.

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
