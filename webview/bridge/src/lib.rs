//! La face réseau locale de la couche navigateur : le protocole JSON-RPC 2.0 et les noms
//! `ego.*`, indépendamment du moteur qui les exécutera.

pub mod protocol;
pub mod server;

pub use protocol::{Call, ErrorObject, Id, JSON_RPC, Reply, Request, rpc};
