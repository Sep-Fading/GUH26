# MiniClaw: lets asyncio programs run inside MiniClaw's Windows sandbox.
#
# asyncio's Windows event loop wakes itself through a socket pair (Lib/asyncio/proactor_events.py,
# _make_self_pipe), and CPython on Windows builds that pair from a TCP listener on 127.0.0.1
# (Lib/socket.py, _fallback_socketpair). Its subprocess support uses named pipes with global names
# (Lib/asyncio/windows_utils.py, pipe). The sandbox refuses both: every listener, and every pipe
# name outside the run's own namespace. So no asyncio program, and with it nearly no Python MCP
# server, could start there.
#
# For the Python programs MiniClaw starts in its sandbox, it puts this file's folder first on
# PYTHONPATH and sets MINICLAW_PYTHON_SANDBOX=1. Python imports this file at startup (Lib/site.py).
# When asyncio is imported later, two of its functions are replaced: the event loop's wake-up
# channel becomes a named pipe, and subprocess pipes get names in the run's own namespace
# (\\.\pipe\LOCAL\, which Windows places under the sandbox's package for a sandboxed process).
# Neither opens a network socket, a port or a file.
#
# This grants nothing: it runs inside the sandbox with the program's own rights. Without
# MINICLAW_PYTHON_SANDBOX, or on another system, it changes nothing. In every case it then runs
# the sitecustomize module that this file hides, if the interpreter has one.
import os
import sys

MARKER = "MINICLAW_PYTHON_SANDBOX"
PIPE_PREFIX = "\\\\.\\pipe\\LOCAL\\python-pipe-"


def pipe(*, duplex=False, overlapped=(True, True), bufsize=8192):
    """Like asyncio.windows_utils.pipe, with the pipe's name in the process's own namespace."""
    import _winapi
    if duplex:
        mode, access, sizes = _winapi.PIPE_ACCESS_DUPLEX, _winapi.GENERIC_READ | _winapi.GENERIC_WRITE, (bufsize, bufsize)
    else:
        mode, access, sizes = _winapi.PIPE_ACCESS_INBOUND, _winapi.GENERIC_WRITE, (0, bufsize)
    mode |= _winapi.FILE_FLAG_FIRST_PIPE_INSTANCE | (_winapi.FILE_FLAG_OVERLAPPED if overlapped[0] else 0)
    name = PIPE_PREFIX + "%d-%s" % (os.getpid(), os.urandom(16).hex())
    server = client = None
    try:
        server = _winapi.CreateNamedPipe(name, mode, _winapi.PIPE_WAIT, 1, sizes[0], sizes[1], _winapi.NMPWAIT_WAIT_FOREVER, _winapi.NULL)
        client = _winapi.CreateFile(name, access, 0, _winapi.NULL, _winapi.OPEN_EXISTING,
                                    _winapi.FILE_FLAG_OVERLAPPED if overlapped[1] else 0, _winapi.NULL)
        _winapi.ConnectNamedPipe(server, overlapped=True).GetOverlappedResult(True)
        return server, client
    except BaseException:
        for handle in (server, client):
            if handle is not None:
                _winapi.CloseHandle(handle)
        raise


class WakeEnd:
    """The writing end of the wake-up pipe, where asyncio expects the socket it writes to."""

    def __init__(self, handle):
        import _winapi
        # Never blocks: a full pipe already holds a pending wake-up.
        _winapi.SetNamedPipeHandleState(handle, 0x1, None, None)  # PIPE_NOWAIT
        self._handle = handle

    def fileno(self):
        # The loop passes this to signal.set_wakeup_fd, which accepts only a socket or a C file
        # descriptor and probes a descriptor with getsockopt, which a sandboxed process must not
        # do (measured: the process is stopped with STATUS_INVALID_HANDLE). -1 leaves the loop
        # without a wake-up on Ctrl+C, which MiniClaw's runs do not use.
        return -1

    def send(self, data):
        import _winapi
        if self._handle is None:
            raise OSError("The wake-up pipe is closed")
        return _winapi.WriteFile(self._handle, data)[0]

    def close(self):
        import _winapi
        if self._handle is not None:
            handle, self._handle = self._handle, None
            _winapi.CloseHandle(handle)


def make_self_pipe(loop):
    """Replaces BaseProactorEventLoop._make_self_pipe: the loop reads one end through its
    completion port (a handle that is not a socket is read with ReadFile) and others wake it
    by writing to the other end."""
    from asyncio import windows_utils
    reader, writer = pipe(overlapped=(True, False))
    try:
        loop._ssock = windows_utils.PipeHandle(reader)
        loop._csock = WakeEnd(writer)
    except BaseException:
        import _winapi
        _winapi.CloseHandle(reader)
        _winapi.CloseHandle(writer)
        raise
    loop._internal_fds += 1


PATCHES = {
    "asyncio.windows_utils": lambda module: setattr(module, "pipe", pipe),
    "asyncio.proactor_events": lambda module: setattr(module.BaseProactorEventLoop, "_make_self_pipe", make_self_pipe),
}


class Patcher:
    """Applies PATCHES right after each module is first imported."""

    def find_spec(self, name, path=None, target=None):
        if name not in PATCHES:
            return None
        import importlib.machinery
        spec = importlib.machinery.PathFinder.find_spec(name, path)
        loader = getattr(spec, "loader", None)
        if loader is None or not hasattr(loader, "exec_module"):
            return spec
        execute = loader.exec_module

        def exec_module(module):
            execute(module)
            PATCHES[name](module)

        loader.exec_module = exec_module
        return spec

    def invalidate_caches(self):
        pass


def install(platform=sys.platform, environ=os.environ):
    """Installs the patches for a sandboxed Windows Python; returns whether it did."""
    if platform != "win32" or environ.get(MARKER) != "1":
        return False
    sys.meta_path.insert(0, Patcher())
    return True


def run_hidden_sitecustomize():
    """Runs the sitecustomize module the interpreter would have run without this file."""
    import importlib.machinery
    import importlib.util
    here = os.path.normcase(os.path.dirname(os.path.abspath(__file__)))
    path = [entry for entry in sys.path if os.path.normcase(os.path.abspath(entry or os.curdir)) != here]
    spec = importlib.machinery.PathFinder.find_spec("sitecustomize", path)
    if spec is None or spec.loader is None:
        return
    module = importlib.util.module_from_spec(spec)
    sys.modules["sitecustomize"] = module
    spec.loader.exec_module(module)


if __name__ == "sitecustomize":
    # Stays reachable under its own name after the hidden module takes "sitecustomize".
    sys.modules["miniclaw_sitecustomize"] = sys.modules[__name__]
    try:
        install()
    except Exception:
        # Never stops the interpreter: without the patches asyncio fails as it did before.
        pass
    run_hidden_sitecustomize()
