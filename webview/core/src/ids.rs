use serde::{Deserialize, Serialize};

/// Identifiant d'un space (l'unité de travail d'un agent : un ensemble d'onglets et un profil).
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct SpaceId(pub u64);

/// Identifiant d'un onglet, c'est-à-dire d'une `WebView` du moteur.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
pub struct TabId(pub u64);

/// Identifiant d'un profil persistant (`Opts::config_dir`).
#[derive(Clone, Debug, Eq, Hash, PartialEq, Deserialize, Serialize)]
pub struct ProfileId(pub String);
