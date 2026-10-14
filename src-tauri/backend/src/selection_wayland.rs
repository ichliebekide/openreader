use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Write},
    os::fd::{AsFd, FromRawFd},
    sync::mpsc::{self, Sender},
    thread,
};

use wayland_client::{
    globals::{registry_queue_init, GlobalListContents},
    protocol::{wl_registry, wl_seat::WlSeat},
    Connection, Dispatch, Proxy, QueueHandle,
};
use wayland_protocols::ext::data_control::v1::client::{
    ext_data_control_device_v1::{self, ExtDataControlDeviceV1},
    ext_data_control_manager_v1::ExtDataControlManagerV1,
    ext_data_control_offer_v1::{self, ExtDataControlOfferV1},
};

const MAX_SELECTION_BYTES: usize = 2 * 1024 * 1024;

struct SelectionState {
    mime_types: HashMap<wayland_client::backend::ObjectId, Vec<String>>,
    output: Sender<File>,
    suppress_initial: bool,
    running: bool,
}

pub fn run() -> Result<(), String> {
    set_parent_death_signal()?;

    let connection = Connection::connect_to_env().map_err(|error| error.to_string())?;
    let (globals, mut event_queue) =
        registry_queue_init::<SelectionState>(&connection).map_err(|error| error.to_string())?;
    let queue_handle = event_queue.handle();

    let manager: ExtDataControlManagerV1 = globals
        .bind(&queue_handle, 1..=1, ())
        .map_err(|error| format!("ext-data-control-v1 unavailable: {error}"))?;
    let seat: WlSeat = globals
        .bind(&queue_handle, 1..=9, ())
        .map_err(|error| format!("Wayland seat unavailable: {error}"))?;

    let (output, selections) = mpsc::channel();
    thread::spawn(move || write_selections(selections));

    let mut state = SelectionState {
        mime_types: HashMap::new(),
        output,
        suppress_initial: true,
        running: true,
    };
    let _device = manager.get_data_device(&seat, &queue_handle, ());

    while state.running {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

fn set_parent_death_signal() -> Result<(), String> {
    if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if unsafe { libc::getppid() } == 1 {
        return Err("selection helper parent already exited".to_string());
    }
    Ok(())
}

fn write_selections(selections: mpsc::Receiver<File>) {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();

    for mut selection in selections {
        let mut bytes = Vec::new();
        if Read::by_ref(&mut selection)
            .take((MAX_SELECTION_BYTES + 1) as u64)
            .read_to_end(&mut bytes)
            .is_err()
            || bytes.len() > MAX_SELECTION_BYTES
        {
            continue;
        }

        if output.write_all(&bytes).is_err()
            || output.write_all(&[0]).is_err()
            || output.flush().is_err()
        {
            break;
        }
    }
}

fn preferred_mime_type(mime_types: &[String]) -> Option<&str> {
    const PREFERRED: [&str; 4] = [
        "text/plain;charset=utf-8",
        "text/plain",
        "UTF8_STRING",
        "STRING",
    ];

    PREFERRED
        .iter()
        .find(|candidate| mime_types.iter().any(|mime| mime == **candidate))
        .copied()
        .or_else(|| {
            mime_types
                .iter()
                .find(|mime| mime.starts_with("text/"))
                .map(String::as_str)
        })
}

fn request_selection(
    state: &mut SelectionState,
    offer: ExtDataControlOfferV1,
) -> Result<(), String> {
    let mime_types = state.mime_types.remove(&offer.id()).unwrap_or_default();
    if state.suppress_initial {
        state.suppress_initial = false;
        offer.destroy();
        return Ok(());
    }

    let Some(mime_type) = preferred_mime_type(&mime_types) else {
        offer.destroy();
        return Ok(());
    };

    let mut file_descriptors = [0; 2];
    if unsafe { libc::pipe2(file_descriptors.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        offer.destroy();
        return Err(std::io::Error::last_os_error().to_string());
    }

    let reader = unsafe { File::from_raw_fd(file_descriptors[0]) };
    let writer = unsafe { File::from_raw_fd(file_descriptors[1]) };
    offer.receive(mime_type.to_string(), writer.as_fd());
    drop(writer);
    offer.destroy();

    state.output.send(reader).map_err(|error| error.to_string())
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for SelectionState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
    }
}

wayland_client::delegate_noop!(SelectionState: ignore WlSeat);
wayland_client::delegate_noop!(SelectionState: ignore ExtDataControlManagerV1);

impl Dispatch<ExtDataControlOfferV1, ()> for SelectionState {
    fn event(
        state: &mut Self,
        offer: &ExtDataControlOfferV1,
        event: ext_data_control_offer_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        if let ext_data_control_offer_v1::Event::Offer { mime_type } = event {
            state
                .mime_types
                .entry(offer.id())
                .or_default()
                .push(mime_type);
        }
    }
}

impl Dispatch<ExtDataControlDeviceV1, ()> for SelectionState {
    fn event(
        state: &mut Self,
        _device: &ExtDataControlDeviceV1,
        event: ext_data_control_device_v1::Event,
        _data: &(),
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
    ) {
        match event {
            ext_data_control_device_v1::Event::Selection { id: Some(offer) } => {
                state.mime_types.remove(&offer.id());
                offer.destroy();
            }
            ext_data_control_device_v1::Event::PrimarySelection { id: Some(offer) } => {
                if let Err(error) = request_selection(state, offer) {
                    eprintln!("OpenReader selection transfer failed: {error}");
                }
            }
            ext_data_control_device_v1::Event::PrimarySelection { id: None } => {
                state.suppress_initial = false;
            }
            ext_data_control_device_v1::Event::Finished => state.running = false,
            _ => {}
        }
    }

    wayland_client::event_created_child!(SelectionState, ExtDataControlDeviceV1, [
        ext_data_control_device_v1::EVT_DATA_OFFER_OPCODE => (ExtDataControlOfferV1, ()),
    ]);
}
