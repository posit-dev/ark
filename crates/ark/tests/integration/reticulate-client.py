import sys

from jupyter_client import BlockingKernelClient

client = BlockingKernelClient(connection_file=sys.argv[1])
client.load_connection_file()
client.start_channels()
try:
    client.wait_for_ready(timeout=20)
    request = client.execute('bundle_answer = 6 * 7; print(bundle_answer)')
    output: list[str] = []
    while True:
        message = client.get_iopub_msg(timeout=20)
        if message['parent_header'].get('msg_id') != request:
            continue
        if message['msg_type'] == 'stream':
            output.append(message['content']['text'])
        if message['msg_type'] == 'error':
            raise RuntimeError(message['content'])
        if message['msg_type'] == 'status' and message['content']['execution_state'] == 'idle':
            break
    reply = client.get_shell_msg(timeout=20)
    assert reply['parent_header']['msg_id'] == request
    assert reply['content']['status'] == 'ok'
    assert ''.join(output) == '42\n'
    print('Jupyter execute_request returned 42')
finally:
    client.stop_channels()
