"""Calls evmvx_test_precompile in build/libevmvx.so."""
import ctypes
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
_lib = ctypes.CDLL(str(ROOT / 'build' / 'libevmvx.so'))
_fn = _lib.evmvx_test_precompile
_fn.argtypes = [ctypes.c_uint64, ctypes.c_uint64, ctypes.c_char_p, ctypes.c_uint64, ctypes.c_uint64,
                ctypes.c_char_p, ctypes.POINTER(ctypes.c_uint64), ctypes.POINTER(ctypes.c_uint64)]
_fn.restype = ctypes.c_uint64

PC_OK, PC_OOG, PC_ERROR, PC_UNIMPLEMENTED = 0, 1, 2, 3


def run_precompile(pid, data, gas_limit, max_out=1 << 16, spec=13):
    """Returns (status, output bytes, gas used)."""
    out = ctypes.create_string_buffer(max_out)
    n = ctypes.c_uint64(0)
    g = ctypes.c_uint64(0)
    st = _fn(spec, pid, data, len(data), gas_limit, out, ctypes.byref(n), ctypes.byref(g))
    return st, out.raw[:n.value], g.value
