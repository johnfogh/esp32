//! Host-side BLE device manager, virtual sensors, and devices.json.
pub mod central;
pub mod devices;
pub mod linked;
pub mod local;
pub mod protocol;
pub mod roster;
pub mod sensor;

pub use central::{BleCmd, BleEvent, DeviceInfo, Sender};
pub use devices::Devices;
pub use linked::{LastMsg, Linked};
pub use sensor::{
    Board, Hub, Kind, Link, Snapshot, ZoneHit, AccelUnit, CHANNELS, PROJECT, SENSOR_COUNT, THRESHOLD,
};
