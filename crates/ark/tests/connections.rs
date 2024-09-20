use amalthea::comm::comm_channel::CommMsg;
use amalthea::comm::connections_comm::ConnectionsBackendReply;
use amalthea::comm::connections_comm::ConnectionsBackendRequest;
use amalthea::comm::connections_comm::ConnectionsFrontendEvent;
use amalthea::comm::connections_comm::ContainsDataParams;
use amalthea::comm::connections_comm::FieldSchema;
use amalthea::comm::connections_comm::GetIconParams;
use amalthea::comm::connections_comm::ListFieldsParams;
use amalthea::comm::connections_comm::ListObjectsParams;
use amalthea::comm::connections_comm::ObjectSchema;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket;
use ark::connections::r_connection::Metadata;
use ark::connections::r_connection::RConnection;
use ark::fixtures::r_test_init;
use ark::fixtures::socket_rpc_request;
use ark::modules::ARK_ENVS;
use ark::r_task::r_task;
use crossbeam::channel::bounded;
use harp::exec::RFunction;
use harp::object::RObject;
use stdext::assert_match;

fn open_dummy_connection() -> socket::comm::CommSocket {
    print!("testing!\n");

    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);

    let comm_id = r_task(|| unsafe {
        let mut dummy_connection = RFunction::new("", ".ps.register_dummy_connection");
        let comm_id = dummy_connection.call_in(ARK_ENVS.positron_ns)?;
        RObject::to::<String>(comm_id)
    })
    .unwrap();

    // R returns the comm socket id that's used as key to communicate with the comm.
    // but it didn't actually open the comm because RMain is not initialized in tests
    // thus we need to manually open the comm here, using our own CommManager.
    // we run this in a spare thread because it will block until we read the messsage
    stdext::spawn!("start-connection-thread", {
        let id = comm_id.clone();
        move || {
            let metadata = Metadata {
                name: String::from("Dummy conn"),
                host: Some(String::from("Dummy host")),
                r#type: Some(String::from("Dummy type")),
                code: Some(String::from("Dummy connect code")),
                language_id: String::from("r"),
            };

            RConnection::start(metadata, comm_manager_tx, id)
        }
    });

    // Wait for the new comm to show up.
    let msg = comm_manager_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    match msg {
        CommManagerEvent::Opened(socket, _value) => {
            assert_eq!(socket.comm_name, "positron.connection");
            assert_eq!(socket.comm_id, comm_id);
            socket
        },
        _ => panic!("Unexpected Comm Manager Event"),
    }
}

fn obj(name: &str, kind: &str) -> ObjectSchema {
    ObjectSchema {
        name: String::from(name),
        kind: String::from(kind),
    }
}

fn field(name: &str, dtype: &str) -> FieldSchema {
    FieldSchema {
        name: String::from(name),
        dtype: String::from(dtype),
    }
}

fn socket_rpc(
    socket: &socket::comm::CommSocket,
    req: ConnectionsBackendRequest,
) -> ConnectionsBackendReply {
    socket_rpc_request::<ConnectionsBackendRequest, ConnectionsBackendReply>(&socket, req)
}

#[test]
fn test_connections_get_icon() {
    r_test_init();
    let socket = open_dummy_connection();

    // Check that we get the correct icons
    let cases: Vec<(Vec<ObjectSchema>, String)> = vec![
        (vec![], "dummy-connection.png".to_string()),
        (vec![obj("main", "schema")], "schema.png".to_string()),
        (
            vec![obj("main", "schema"), obj("table1", "table")],
            "table.png".to_string(),
        ),
        (
            vec![obj("main", "schema"), obj("table2", "table")],
            "table.png".to_string(),
        ),
        (
            vec![obj("main", "schema"), obj("view1", "view")],
            "".to_string(),
        ),
    ];

    for (path, icon_path) in cases {
        assert_match!(
            socket_rpc(&socket, ConnectionsBackendRequest::GetIcon(GetIconParams { path })),
            ConnectionsBackendReply::GetIconReply(path) => {
                assert_eq!(path, icon_path);
            }
        );
    }
}

#[test]
fn test_connections_contains_data() {
    r_test_init();
    let socket = open_dummy_connection();

    // Check that we get the correct `contains_data`
    let cases: Vec<(Vec<ObjectSchema>, bool)> = vec![
        (vec![], false),
        (vec![obj("main", "schema")], false),
        (vec![obj("main", "schema"), obj("table1", "table")], true),
        (vec![obj("main", "schema"), obj("table2", "table")], true),
        (vec![obj("main", "schema"), obj("view1", "view")], true),
    ];

    for (path, contains_data) in cases {
        assert_match!(
            socket_rpc(&socket, ConnectionsBackendRequest::ContainsData(ContainsDataParams { path })),
            ConnectionsBackendReply::ContainsDataReply(val) => {
                assert_eq!(val, contains_data);
            }
        );
    }
}

#[test]
fn test_connections_list_objects() {
    r_test_init();
    let socket = open_dummy_connection();

    // Check that we get the correct list of objects
    let cases: Vec<(Vec<ObjectSchema>, Vec<ObjectSchema>)> = vec![
        (vec![], vec![obj("main", "schema")]),
        (vec![obj("main", "schema")], vec![
            obj("table1", "table"),
            obj("table2", "table"),
            obj("view1", "view"),
        ]),
    ];

    for (path, objects) in cases {
        assert_match!(
            socket_rpc(&socket, ConnectionsBackendRequest::ListObjects(ListObjectsParams { path })),
            ConnectionsBackendReply::ListObjectsReply(val) => {
                assert_eq!(val, objects);
            }
        );
    }
}

#[test]
fn test_connection_list_fields() {
    r_test_init();
    let socket = open_dummy_connection();

    // Check that we get the correct list of objects
    let cases: Vec<(Vec<ObjectSchema>, Vec<FieldSchema>)> = vec![
        (vec![obj("main", "schema"), obj("table1", "table")], vec![
            field("table1_col1", "integer"),
            field("table1_col2", "character"),
            field("table1_col3", "logical"),
        ]),
        (vec![obj("main", "schema"), obj("view1", "view")], vec![
            field("view1_col1", "integer"),
            field("view1_col2", "character"),
            field("view1_col3", "logical"),
        ]),
    ];

    for (path, objects) in cases {
        assert_match!(
            socket_rpc(&socket, ConnectionsBackendRequest::ListFields(ListFieldsParams { path })),
            ConnectionsBackendReply::ListFieldsReply(val) => {
                assert_eq!(val, objects);
            }
        );
    }
}

#[test]
fn test_send_frontend_event() {
    r_test_init();
    let socket = open_dummy_connection();

    let event = ConnectionsFrontendEvent::Update;

    socket
        .incoming_tx
        .send(CommMsg::Data(serde_json::to_value(event).unwrap()))
        .unwrap();

    let msg = socket
        .outgoing_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .unwrap();

    if let CommMsg::Data(value) = msg {
        let v: ConnectionsFrontendEvent = serde_json::from_value(value).unwrap();
        assert_eq!(ConnectionsFrontendEvent::Update, v);
    } else {
        panic!("Expected a CommMsg::Data");
    }
}
