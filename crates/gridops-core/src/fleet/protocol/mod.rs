//! Version-one fleet transport contracts shared by the gateway and host agent.
//! Parsing preserves units and bounds; it does not authenticate a peer or prove
//! that a remote operation happened. Those checks belong to the durable service.

pub mod agent;
pub mod browser;
pub mod chunks;
pub mod events;
pub mod inventory;
pub mod limits;
pub mod outcomes;
pub mod primitives;
pub mod renewal;
