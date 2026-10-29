#!/usr/bin/env python3
"""Real PTY smoke. Requires Unix and a built release binary; no external packages."""
import errno, fcntl, os, pathlib, pty, select, signal, struct, subprocess, tempfile, termios, time, sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
BINARY = pathlib.Path(sys.argv[1]).resolve() if len(sys.argv) > 1 else ROOT / 'target/release/mdir'

def drain(master, wait=0.12):
    data = b''
    deadline = time.monotonic() + wait
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master], [], [], max(0, deadline-time.monotonic()))
        if ready:
            try:
                chunk = os.read(master, 65536)
                if not chunk: break
                data += chunk
            except OSError as error:
                if error.errno == errno.EIO: break
                raise
    return data

def send(master, value):
    os.write(master, value)
    return drain(master, 0.2)

def wait_for_screen(master, expected, timeout=5):
    output = b''
    deadline = time.monotonic() + timeout
    while expected not in output and time.monotonic() < deadline:
        output += drain(master, 0.1)
    assert expected in output, output
    # Initial worker results should settle before checking for idle output.
    output += drain(master, 0.25)
    return output

def session(left, right, quit_key=b'q', actions=False):
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 110, 0, 0))
    before = termios.tcgetattr(slave)
    env = dict(os.environ, TERM='xterm-256color')
    process = subprocess.Popen([str(BINARY), '--ascii', '--mono', str(left), str(right)], stdin=slave, stdout=slave, stderr=slave, env=env)
    try:
        screen = wait_for_screen(master, b'mDir')
        assert not drain(master, 0.25), 'unexpected idle redraw'
        if actions:
            send(master, b'c')
            assert not (right / '한글.txt').exists(), 'copy executed before confirmation'
            send(master, b'n')
            assert not (right / '한글.txt').exists(), 'cancelled copy executed'
            send(master, b'c'); send(master, b'y')
            assert (right / '한글.txt').read_text() == 'line one\nline two\n'
            send(master, b'p'); send(master, b'\x1b[6~'); send(master, b'q')
            send(master, b'n'); send(master, '새폴더'.encode()); send(master, b'\r'); send(master, b'y')
            assert (left / '새폴더').is_dir()
            fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 20, 80, 0, 0))
            process.send_signal(signal.SIGWINCH)
            drain(master)
        screen += send(master, quit_key)
        assert process.wait(timeout=3) == 0
        after = termios.tcgetattr(slave)
        assert after == before, 'terminal attributes not restored'
        assert b'\x1b[?1049l' in screen, 'alternate screen not restored'
    finally:
        if process.poll() is None: process.kill(); process.wait()
        os.close(master); os.close(slave)

def extended_session(root):
    left, right = root/'extended-left', root/'extended-right'
    left.mkdir(); right.mkdir()
    (left/'a.txt').write_text('private')
    (left/'a.txt').chmod(0o600)
    (left/'b.txt').write_text('second')
    (left/'long.txt').write_text('HEAD' + 'x'*10000 + 'TAIL_SENTINEL')
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 28, 110, 0, 0))
    before = termios.tcgetattr(slave)
    process = subprocess.Popen([str(BINARY), '--ascii', '--mono', str(left), str(right)], stdin=slave, stdout=slave, stderr=slave, env=dict(os.environ, TERM='xterm-256color'))
    try:
        wait_for_screen(master, b'mDir')
        send(master,b'  ')
        review=send(master,b'c')
        assert b'a.txt' in review and b'b.txt' in review
        assert not list(right.iterdir())
        send(master,b'y')
        assert (right/'a.txt').read_text()=='private' and (right/'b.txt').read_text()=='second'
        assert (right/'a.txt').stat().st_mode & 0o777 == 0o600
        send(master,b'/b.txt\r')
        send(master,b'e');send(master,'이름변경.txt'.encode());send(master,b'\r');send(master,b'y')
        assert not (left/'b.txt').exists() and (left/'이름변경.txt').read_text()=='second'
        send(master,b'/\x1b')
        send(master,b'/long.txt\r');send(master,b'p')
        end=send(master,b'\x1b[F')
        assert b'TAIL_SENTINEL' in end, 'wrapped preview tail is unreachable'
        send(master,b'q');send(master,b'/\x1b')
        send(master,b'/a.txt\r');send(master,b'c');send(master,b'y')
        assert (right/'a.txt').read_text()=='private', 'collision changed target'
        send(master,b'd');send(master,b'n')
        assert (left/'a.txt').exists(), 'cancelled delete changed source'
        send(master,b'd');send(master,b'y')
        assert not (left/'a.txt').exists()
        send(master,b'g');send(master,str(right).encode());send(master,b'\r')
        send(master,b'n');send(master,b'newdir\r');send(master,b'y')
        assert (right/'newdir').is_dir()
        send(master,b'w');send(master,b'\t');send(master,b'\t')
        send(master,b't');send(master,b't')
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 8, 20, 0, 0));process.send_signal(signal.SIGWINCH)
        drain(master)
        send(master,b'q')
        assert process.wait(timeout=3)==0
        assert termios.tcgetattr(slave)==before
    finally:
        if process.poll() is None: process.kill();process.wait()
        os.close(master);os.close(slave)

with tempfile.TemporaryDirectory(prefix='mdir-smoke-') as temporary:
    root = pathlib.Path(temporary)
    left, right = root/'left', root/'right'
    left.mkdir(); right.mkdir()
    (left/'한글.txt').write_text('line one\nline two\n')
    session(left, right, actions=True)
    session(left, right, quit_key=b'\x03')
    extended_session(root)
print('PASS: Unicode paths, multiselect, copy/collision/modes, rename, delete/cancel, goto, wrapped preview, layouts, resize, idle, terminal restoration')
