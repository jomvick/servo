//! Le modèle du navigateur, sans moteur et sans fenêtre : spaces, onglets, qui a la main,
//! et les erreurs que l'API `ego.*` publie.

mod error;
mod ids;
mod snapshot;
mod space;
mod tab;

use std::collections::HashMap;

pub use error::{EgoError, ErrorCode};
pub use ids::{ProfileId, SpaceId, TabId};
pub use snapshot::{RefEntry, Snapshot, SnapshotOptions, parse_snapshot};
pub use space::{Ownership, Space};
pub use tab::Tab;

pub const CRATE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default)]
pub struct Registry {
    spaces: HashMap<SpaceId, Space>,
    tabs: HashMap<TabId, Tab>,
    next_space: u64,
    next_tab: u64,
}

impl Registry {
    /// Un space naît sous main d'agent : c'est l'unité de travail d'un agent. Un space créé
    /// depuis la fenêtre appelle `hand_off` juste après.
    pub fn create_space(&mut self, name: &str, profile: ProfileId) -> SpaceId {
        self.next_space += 1;
        let id = SpaceId(self.next_space);
        self.spaces.insert(
            id,
            Space {
                id,
                name: name.to_owned(),
                profile,
                tabs: Vec::new(),
                active: None,
                ownership: Ownership::Agent,
            },
        );
        id
    }

    pub fn space(&self, id: SpaceId) -> Result<&Space, EgoError> {
        self.spaces.get(&id).ok_or_else(|| space_not_found(id))
    }

    pub fn tab(&self, id: TabId) -> Result<&Tab, EgoError> {
        self.tabs.get(&id).ok_or_else(|| tab_unavailable(id))
    }

    /// L'adresse du moteur : l'URL, le titre et l'état de chargement d'un onglet sont ce que
    /// le moteur observe, donc c'est lui qui les écrit.
    pub fn tab_mut(&mut self, id: TabId) -> Result<&mut Tab, EgoError> {
        self.tabs.get_mut(&id).ok_or_else(|| tab_unavailable(id))
    }

    /// L'onglet que la fenêtre montre. Le modèle reste autoritaire là-dessus, y compris pour
    /// une sélection venue de l'utilisateur.
    pub fn set_active_tab(&mut self, id: TabId) -> Result<(), EgoError> {
        let space = self.tab(id)?.space;
        let space = self
            .spaces
            .get_mut(&space)
            .expect("a tab outlives its space");
        space.active = Some(id);
        Ok(())
    }

    pub fn spaces(&self) -> impl Iterator<Item = &Space> {
        self.spaces.values()
    }

    pub fn tabs(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.values()
    }

    /// Le garde-fou des opérations agent : il est appelé par le thread moteur avant toute
    /// commande d'agent, jamais par le pont.
    pub fn require_agent_control(&self, id: SpaceId) -> Result<&Space, EgoError> {
        let space = self.space(id)?;
        match space.ownership {
            Ownership::Agent => Ok(space),
            Ownership::User => Err(EgoError::new(
                ErrorCode::TaskSpaceUserInControl,
                format!("the user holds task space {}", id.0),
            )),
            Ownership::Done => Err(space_completed(id)),
        }
    }

    pub fn create_tab(&mut self, space: SpaceId, url: Option<String>) -> Result<TabId, EgoError> {
        self.require_agent_control(space)?;
        self.next_tab += 1;
        let id = TabId(self.next_tab);
        self.tabs.insert(
            id,
            Tab {
                id,
                space,
                url,
                title: None,
                loading: true,
            },
        );
        let space = self.spaces.get_mut(&space).expect("checked above");
        space.tabs.push(id);
        if space.active.is_none() {
            space.active = Some(id);
        }
        Ok(id)
    }

    /// Fermer un onglet. Les suppressions ne sont pas gardées par la propriété : l'utilisateur
    /// doit pouvoir fermer un onglet d'un space qu'il tient, comme l'agent refermer un space
    /// complété. Seules les mutations qui donnent du travail à l'agent exigent `Agent`.
    pub fn close_tab(&mut self, id: TabId) -> Result<(), EgoError> {
        let tab = self.tab(id)?.clone();
        self.tabs.remove(&id);
        let space = self
            .spaces
            .get_mut(&tab.space)
            .expect("a tab outlives its space");
        space.tabs.retain(|t| *t != id);
        if space.active == Some(id) {
            space.active = space.tabs.last().copied();
        }
        Ok(())
    }

    pub fn claim(&mut self, id: SpaceId) -> Result<(), EgoError> {
        self.set_ownership(id, Ownership::Agent)
    }

    /// La main passe à l'utilisateur. Depuis `User`, idempotent.
    pub fn hand_off(&mut self, id: SpaceId) -> Result<(), EgoError> {
        self.set_ownership(id, Ownership::User)
    }

    pub fn take_over(&mut self, id: SpaceId) -> Result<(), EgoError> {
        self.set_ownership(id, Ownership::Agent)
    }

    pub fn complete(&mut self, id: SpaceId) -> Result<(), EgoError> {
        self.set_ownership(id, Ownership::Done)
    }

    /// Supprime le space et tous ses onglets, et rend les onglets supprimés pour que le
    /// moteur libère les `WebView` correspondantes.
    pub fn close(&mut self, id: SpaceId) -> Result<Vec<TabId>, EgoError> {
        let space = self.spaces.remove(&id).ok_or_else(|| space_not_found(id))?;
        for tab in &space.tabs {
            self.tabs.remove(tab);
        }
        Ok(space.tabs)
    }

    fn set_ownership(&mut self, id: SpaceId, ownership: Ownership) -> Result<(), EgoError> {
        let space = self
            .spaces
            .get_mut(&id)
            .ok_or_else(|| space_not_found(id))?;
        if space.ownership == Ownership::Done {
            return Err(space_completed(id));
        }
        space.ownership = ownership;
        Ok(())
    }
}

fn space_not_found(id: SpaceId) -> EgoError {
    EgoError::new(
        ErrorCode::TaskSpaceNotFound,
        format!("no task space {}", id.0),
    )
}

fn tab_unavailable(id: TabId) -> EgoError {
    EgoError::new(
        ErrorCode::WebContentsUnavailable,
        format!("no tab {}", id.0),
    )
}

fn space_completed(id: SpaceId) -> EgoError {
    EgoError::new(
        ErrorCode::TaskSpaceInactive,
        format!("task space {} is completed", id.0),
    )
}
