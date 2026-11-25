//! The session bus, through zbus: the receiver's `Bus`, the signals it
//! listens for, and the name that keeps the app to one window.

use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::time::Duration;

use zbus::blocking::connection::Builder;
use zbus::blocking::fdo::DBusProxy;
use zbus::blocking::{Connection, MessageIterator};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{MatchRule, Message};

use crate::receiver::{Bus, Val, DBUS_TIMEOUT_MS, WATCHED_OBJECTS, WATCHED_SERVICE};

/// The app's own name on the session bus, which keeps it to one window.
const APP_SERVICE: &str = "org.shairportsync.AirPlayReceiver";
const APP_PATH: &str = "/";
const PROPS_IFACE: &str = "org.freedesktop.DBus.Properties";
const DBUS_SERVICE: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

/// A connection to the session bus on which a call waits `DBUS_TIMEOUT_MS`
/// for its reply; None if there is no bus to connect to.
pub fn connect() -> Option<Connection> {
    Builder::session().ok()?.method_timeout(Duration::from_millis(DBUS_TIMEOUT_MS)).build().ok()
}

fn to_val(value: &Value<'_>) -> Val {
    match value {
        Value::Bool(b) => Val::Bool(*b),
        Value::U8(n) => Val::UInt(u64::from(*n)),
        Value::U16(n) => Val::UInt(u64::from(*n)),
        Value::U32(n) => Val::UInt(u64::from(*n)),
        Value::U64(n) => Val::UInt(*n),
        Value::I16(n) => Val::Int(i64::from(*n)),
        Value::I32(n) => Val::Int(i64::from(*n)),
        Value::I64(n) => Val::Int(*n),
        Value::F64(n) => Val::Double(*n),
        Value::Str(s) => Val::Str(s.as_str().to_string()),
        Value::ObjectPath(path) => Val::Str(path.as_str().to_string()),
        Value::Signature(signature) => Val::Str(signature.to_string()),
        Value::Value(inner) => to_val(inner),
        Value::Array(array) => {
            let items = array.inner();
            if items.iter().all(|item| matches!(item, Value::U8(_))) {
                Val::Bytes(items.iter().filter_map(|item| u8::try_from(item).ok()).collect())
            } else {
                Val::List(items.iter().map(to_val).collect())
            }
        }
        Value::Structure(structure) => Val::List(structure.fields().iter().map(to_val).collect()),
        Value::Dict(dict) => Val::Dict(
            dict.iter()
                .filter_map(|(key, value)| match to_val(key) {
                    Val::Str(key) => Some((key, to_val(value))),
                    _ => None,
                })
                .collect(),
        ),
        _ => Val::Other,
    }
}

/// What a reply holds, for the methods whose replies the app reads.
fn reply_values(method: &str, reply: &Message) -> Vec<Val> {
    let body = reply.body();
    let value = match method {
        "GetAll" => body.deserialize::<HashMap<String, OwnedValue>>().ok().map(|props| {
            Val::Dict(props.iter().map(|(key, value)| (key.clone(), to_val(value))).collect())
        }),
        "Get" => body.deserialize::<OwnedValue>().ok().map(|value| to_val(&value)),
        "LoadUnit" => body.deserialize::<OwnedObjectPath>().ok().map(|path| Val::Str(path.as_str().to_string())),
        _ => None,
    };
    value.into_iter().collect()
}

/// The receiver's `Bus`.
pub struct SessionBus(pub Option<Connection>);

impl Bus for SessionBus {
    fn call(&self, service: &str, path: &str, interface: &str, method: &str, args: &[Val]) -> Result<Vec<Val>, String> {
        let Some(connection) = &self.0 else {
            return Err("Not connected to D-Bus server".to_string());
        };
        let (service, interface) = (Some(service), Some(interface));
        let reply = match args {
            [] => connection.call_method(service, path, interface, method, &()),
            [Val::Str(a)] => connection.call_method(service, path, interface, method, &(a.as_str(),)),
            [Val::Str(a), Val::Str(b)] => connection.call_method(service, path, interface, method, &(a.as_str(), b.as_str())),
            [Val::Double(a)] => connection.call_method(service, path, interface, method, &(*a,)),
            _ => return Err(format!("{method}: arguments of these types aren't supported")),
        };
        match reply {
            Ok(reply) => Ok(reply_values(method, &reply)),
            Err(zbus::Error::MethodError(name, message, _)) => {
                Err(message.filter(|message| !message.is_empty()).unwrap_or_else(|| name.to_string()))
            }
            Err(zbus::Error::InputOutput(err)) if err.kind() == std::io::ErrorKind::TimedOut => {
                Err("No reply in time".to_string())
            }
            Err(err) => Err(err.to_string()),
        }
    }
}

/// What the bus says that the receiver's state should be read again for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// One of `WATCHED_OBJECTS` changed its properties.
    PropertiesChanged,
    /// `WATCHED_SERVICE` appeared on the bus.
    ServiceRegistered,
    /// `WATCHED_SERVICE` left the bus.
    ServiceUnregistered,
}

fn signal_of(message: &Message) -> Option<Signal> {
    let header = message.header();
    if header.message_type() != Type::Signal {
        return None;
    }
    let path = header.path()?.as_str();
    match header.member()?.as_str() {
        "PropertiesChanged" if WATCHED_OBJECTS.iter().any(|(_, watched)| *watched == path) => {
            Some(Signal::PropertiesChanged)
        }
        "NameOwnerChanged" if path == DBUS_PATH => {
            let (name, _old_owner, new_owner) = message.body().deserialize::<(String, String, String)>().ok()?;
            (name == WATCHED_SERVICE).then_some(if new_owner.is_empty() {
                Signal::ServiceUnregistered
            } else {
                Signal::ServiceRegistered
            })
        }
        _ => None,
    }
}

/// Call `on_signal`, on a thread of its own, for each `Signal` from the bus,
/// until it returns false. The signals come over a connection of their own,
/// so that a burst of them can't hold up the replies to method calls.
pub fn watch(on_signal: impl Fn(Signal) -> bool + Send + 'static) -> zbus::Result<()> {
    let connection = Builder::session()?.build()?;
    let bus = DBusProxy::new(&connection)?;
    for (service, path) in WATCHED_OBJECTS {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(service)?
            .path(path)?
            .interface(PROPS_IFACE)?
            .member("PropertiesChanged")?
            .build();
        bus.add_match_rule(rule)?;
    }
    let rule = MatchRule::builder()
        .msg_type(Type::Signal)
        .sender(DBUS_SERVICE)?
        .path(DBUS_PATH)?
        .interface(DBUS_SERVICE)?
        .member("NameOwnerChanged")?
        .arg(0, WATCHED_SERVICE)?
        .build();
    bus.add_match_rule(rule)?;
    let messages = MessageIterator::from(&connection);
    std::thread::spawn(move || {
        for message in messages {
            let Ok(message) = message else {
                break;
            };
            if signal_of(&message).is_some_and(|signal| !on_signal(signal)) {
                break;
            }
        }
    });
    Ok(())
}

/// Lets a second launch of the app bring this one's window forward.
struct Activator {
    /// Gets the activation token of each launch that asks.
    launches: Sender<String>,
}

#[zbus::interface(name = "org.shairportsync.AirPlayReceiver")]
impl Activator {
    fn activate(&self, token: &str) {
        let _ = self.launches.send(token.to_string());
    }
}

/// Take the app's name on the bus. True if this is the only instance, which
/// then gets the activation token of each later launch through `launches`;
/// false if another one has the name, which has been asked to show its window.
pub fn claim_single_instance(connection: &Connection, launches: Sender<String>) -> bool {
    let _ = connection.object_server().at(APP_PATH, Activator { launches });
    match connection.request_name_with_flags(APP_SERVICE, RequestNameFlags::DoNotQueue.into()) {
        Ok(RequestNameReply::Exists | RequestNameReply::InQueue) | Err(zbus::Error::NameTaken) => {
            let token = std::env::var("XDG_ACTIVATION_TOKEN").unwrap_or_default();
            let _ = connection.call_method(Some(APP_SERVICE), APP_PATH, Some(APP_SERVICE), "Activate", &(token.as_str(),));
            false
        }
        // Without the name, for whatever reason, the app still works.
        _ => true,
    }
}
