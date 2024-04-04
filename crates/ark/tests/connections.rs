use amalthea::comm::connections_comm::ConnectionsBackendReply;
use amalthea::comm::connections_comm::ConnectionsBackendRequest;
use amalthea::comm::connections_comm::ContainsDataParams;
use amalthea::comm::connections_comm::GetIconParams;
use amalthea::comm::connections_comm::ObjectSchema;
use amalthea::comm::event::CommManagerEvent;
use amalthea::socket;
use ark::connections::r_connection::RConnection;
use ark::r_task;
use ark::test::r_test;
use ark::test::socket_rpc_request;
use crossbeam::channel::bounded;
use harp::assert_match;
use harp::exec::RFunction;
use harp::object::RObject;

fn open_dummy_connection() -> socket::comm::CommSocket {
    print!("testign!\n");

    let (comm_manager_tx, comm_manager_rx) = bounded::<CommManagerEvent>(0);

    let comm_id = r_task(|| unsafe {
        let mut dummy_connection = RFunction::new("", ".ps.register_dummy_connection");
        let comm_id = dummy_connection.call()?;
        RObject::to::<String>(comm_id)
    })
    .unwrap();

    // R returns the comm socket id that's used as key to communicate with the comm.
    // but it didn't actually open the comm because RMain is not initialized in tests
    // thus we need to manually open the comm here, using our own CommManager.
    // we run this in a speare theread because it will block until we read the messsage
    stdext::spawn!("start-connection-thread", {
        let id = comm_id.clone();
        move || RConnection::start(String::from("Dummy Comm"), comm_manager_tx, id)
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

fn socket_rpc(
    socket: &socket::comm::CommSocket,
    req: ConnectionsBackendRequest,
) -> ConnectionsBackendReply {
    socket_rpc_request::<ConnectionsBackendRequest, ConnectionsBackendReply>(&socket, req)
}

#[test]
fn test_connections_get_icon() {
    r_test(|| {
        let socket = open_dummy_connection();

        // Check that we get the correct icons
        let cases: Vec<(Vec<ObjectSchema>, String)> = vec![
            (vec![], "dummy-connection.png".to_string()),
            (
                vec![ObjectSchema {
                    name: String::from("main"),
                    kind: String::from("schema"),
                }],
                "schema.png".to_string(),
            ),
            (
                vec![
                    ObjectSchema {
                        name: String::from("main"),
                        kind: String::from("schema"),
                    },
                    ObjectSchema {
                        name: String::from("table1"),
                        kind: String::from("table"),
                    },
                ],
                "table.png".to_string(),
            ),
            (
                vec![
                    ObjectSchema {
                        name: String::from("main"),
                        kind: String::from("schema"),
                    },
                    ObjectSchema {
                        name: String::from("view1"),
                        kind: String::from("view"),
                    },
                ],
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
    })
}

#[test]
fn test_connections_contains_data() {
    r_test(|| {
        let socket = open_dummy_connection();

        // Check that we get the correct contians_data
        let cases: Vec<(Vec<ObjectSchema>, bool)> = vec![
            (vec![], false),
            (
                vec![ObjectSchema {
                    name: String::from("main"),
                    kind: String::from("schema"),
                }],
                false,
            ),
            (
                vec![
                    ObjectSchema {
                        name: String::from("main"),
                        kind: String::from("schema"),
                    },
                    ObjectSchema {
                        name: String::from("table1"),
                        kind: String::from("table"),
                    },
                ],
                true,
            ),
            (
                vec![
                    ObjectSchema {
                        name: String::from("main"),
                        kind: String::from("schema"),
                    },
                    ObjectSchema {
                        name: String::from("view1"),
                        kind: String::from("view"),
                    },
                ],
                true,
            ),
        ];

        for (path, contains_data) in cases {
            assert_match!(
                socket_rpc(&socket, ConnectionsBackendRequest::ContainsData(ContainsDataParams { path })),
                ConnectionsBackendReply::ContainsDataReply(val) => {
                    assert_eq!(val, contains_data);
                }
            );
        }
    })
}
