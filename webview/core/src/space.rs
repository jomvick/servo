use serde::{Deserialize, Serialize};

use crate::ids::{ProfileId, SpaceId, TabId};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub enum Ownership {
    Agent,
    User,
    /// Terminal : plus aucune opération agent n'est admise, seul `close` l'est encore.
    Done,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Space {
    pub id: SpaceId,
    pub name: String,
    pub profile: ProfileId,
    pub tabs: Vec<TabId>,
    pub active: Option<TabId>,
    pub ownership: Ownership,
}
