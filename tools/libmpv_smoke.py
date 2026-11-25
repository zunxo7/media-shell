"""Plays the public HLS test streams through a libmpv build, headless, and fails if any of it doesn't work.

What it proves: the library loads, TLS verifies against the system's trust store (mpv 0.42 has tls-verify on),
libcurl, the HLS demuxer and the decoders work, `hls-bitrate` picks a variant, and the render API hands out frames.
Frames come from the software renderer (MPV_RENDER_API_TYPE_SW), so no GPU or window is needed; the last one of each
stream is saved as a PNG beside the log.

    python3 -I tools/libmpv_smoke.py path/to/libmpv.2.dylib out/dir
"""
import ctypes
import json
import os
import platform
import struct
import sys
import threading
import time
import zlib

STREAMS = [
    ("apple-hevc", "https://devstreaming-cdn.apple.com/videos/streaming/examples/bipbop_adv_example_hevc/master.m3u8"),
    ("mux-h264", "https://test-streams.mux.dev/x36xhzz/x36xhzz.m3u8"),
]

# The app's own options (windows/app/src/player.rs `drive`), with no window and no sound device.
OPTIONS = [
    ("vo", "libmpv"),
    ("ao", "null"),
    ("hwdec", "auto-safe"),
    ("idle", "yes"),
    ("config", "no"),
    ("load-scripts", "no"),
    ("ytdl", "no"),
    ("osc", "no"),
    ("terminal", "no"),
    ("input-default-bindings", "no"),
    ("sid", "no"),
    ("cache", "yes"),
    ("cache-secs", "120"),
    ("demuxer-max-bytes", "400MiB"),
    ("network-timeout", "20"),
    ("hr-seek", "yes"),
]

W, H = 640, 360
STRIDE = W * 4

EV_LOG, EV_END_FILE, EV_FILE_LOADED = 2, 7, 8
API_TYPE, SW_SIZE, SW_FORMAT, SW_STRIDE, SW_POINTER = 1, 17, 18, 19, 20


class Param(ctypes.Structure):
    _fields_ = [("type", ctypes.c_int), ("data", ctypes.c_void_p)]


class Event(ctypes.Structure):
    _fields_ = [("id", ctypes.c_int), ("error", ctypes.c_int), ("reply", ctypes.c_uint64), ("data", ctypes.c_void_p)]


class EndFile(ctypes.Structure):
    _fields_ = [("reason", ctypes.c_int), ("error", ctypes.c_int)]


def load(path):
    lib = ctypes.CDLL(path)
    sig = {
        "mpv_create": ([], ctypes.c_void_p),
        "mpv_initialize": ([ctypes.c_void_p], ctypes.c_int),
        "mpv_set_option_string": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p], ctypes.c_int),
        "mpv_set_property_string": ([ctypes.c_void_p, ctypes.c_char_p, ctypes.c_char_p], ctypes.c_int),
        "mpv_get_property_string": ([ctypes.c_void_p, ctypes.c_char_p], ctypes.c_void_p),
        "mpv_command": ([ctypes.c_void_p, ctypes.POINTER(ctypes.c_char_p)], ctypes.c_int),
        "mpv_wait_event": ([ctypes.c_void_p, ctypes.c_double], ctypes.POINTER(Event)),
        "mpv_free": ([ctypes.c_void_p], None),
        "mpv_error_string": ([ctypes.c_int], ctypes.c_char_p),
        "mpv_terminate_destroy": ([ctypes.c_void_p], None),
        "mpv_client_api_version": ([], ctypes.c_ulong),
        "mpv_render_context_create": ([ctypes.POINTER(ctypes.c_void_p), ctypes.c_void_p, ctypes.POINTER(Param)], ctypes.c_int),
        "mpv_render_context_update": ([ctypes.c_void_p], ctypes.c_uint64),
        "mpv_render_context_render": ([ctypes.c_void_p, ctypes.POINTER(Param)], ctypes.c_int),
        "mpv_render_context_free": ([ctypes.c_void_p], None),
    }
    for name, (args, ret) in sig.items():
        f = getattr(lib, name)
        f.argtypes, f.restype = args, ret
    return lib


class Player:
    def __init__(self, lib, log_file):
        self.lib = lib
        self.h = lib.mpv_create()
        assert self.h, "mpv_create failed"
        for k, v in OPTIONS + [("log-file", log_file)]:
            r = lib.mpv_set_option_string(self.h, k.encode(), v.encode())
            # The app ignores options a build lacks (load-scripts and osc need Lua); the output's must exist.
            assert r >= 0 or k not in ("vo", "ao", "hwdec", "log-file"), f"option {k}={v}: {self.err(r)}"
            if r < 0:
                print(f"option {k}={v}: {self.err(r)} (ignored, as the app does)")
        r = lib.mpv_initialize(self.h)
        assert r >= 0, f"mpv_initialize: {self.err(r)}"
        # The render context has to exist before the first loadfile, or vo=libmpv fails to start video.
        self.ctx = ctypes.c_void_p()
        sw = ctypes.c_char_p(b"sw")
        params = (Param * 2)(Param(API_TYPE, ctypes.cast(sw, ctypes.c_void_p)), Param(0, None))
        r = lib.mpv_render_context_create(ctypes.byref(self.ctx), self.h, params)
        assert r >= 0, f"mpv_render_context_create(sw): {self.err(r)}"
        self.frames = 0
        self.pixels = ctypes.create_string_buffer(STRIDE * H + 64)
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.thread = threading.Thread(target=self.render, daemon=True)
        self.thread.start()

    def err(self, code):
        return self.lib.mpv_error_string(code).decode()

    def render(self):
        # Every mpv_render_* call stays on this thread, as render.h asks; it never calls the rest of the API.
        size = (ctypes.c_int * 2)(W, H)
        fmt = ctypes.c_char_p(b"rgb0")
        stride = ctypes.c_size_t(STRIDE)
        base = ctypes.addressof(self.pixels)
        aligned = (base + 63) & ~63
        params = (Param * 5)(
            Param(SW_SIZE, ctypes.cast(size, ctypes.c_void_p)),
            Param(SW_FORMAT, ctypes.cast(fmt, ctypes.c_void_p)),
            Param(SW_STRIDE, ctypes.cast(ctypes.pointer(stride), ctypes.c_void_p)),
            Param(SW_POINTER, aligned),
            Param(0, None),
        )
        self.offset = aligned - base
        while not self.stop.is_set():
            if self.lib.mpv_render_context_update(self.ctx) & 1:
                with self.lock:
                    if self.lib.mpv_render_context_render(self.ctx, params) >= 0:
                        self.frames += 1
            else:
                time.sleep(0.005)

    def set(self, k, v):
        return self.lib.mpv_set_property_string(self.h, k.encode(), v.encode())

    def get(self, k):
        p = self.lib.mpv_get_property_string(self.h, k.encode())
        if not p:
            return None
        s = ctypes.string_at(p).decode(errors="replace")
        self.lib.mpv_free(p)
        return s

    def num(self, k):
        try:
            return float(self.get(k) or 0)
        except ValueError:
            return 0.0

    def cmd(self, *args):
        arr = (ctypes.c_char_p * (len(args) + 1))(*[a.encode() for a in args], None)
        return self.lib.mpv_command(self.h, arr)

    def play(self, url, bitrate, seconds=5.0, timeout=90.0):
        """Loads `url` at `bitrate` and plays until `seconds` in; what it saw, or an AssertionError."""
        self.set("hls-bitrate", bitrate)
        first = self.frames
        r = self.cmd("loadfile", url, "replace")
        assert r >= 0, f"loadfile: {self.err(r)}"
        deadline = time.time() + timeout
        loaded = False
        while time.time() < deadline:
            ev = self.lib.mpv_wait_event(self.h, 0.25).contents
            if ev.id == EV_FILE_LOADED:
                loaded = True
            elif ev.id == EV_END_FILE and ev.data:
                end = ctypes.cast(ev.data, ctypes.POINTER(EndFile)).contents
                # (reason 2 is the replace of the file before this one)
                if end.reason == 4:
                    raise AssertionError(f"playback failed: {self.err(end.error)}")
            if loaded and self.num("time-pos") > seconds and self.frames > first:
                break
        assert loaded, "no FILE_LOADED in time"
        seen = {
            "time_pos": self.num("time-pos"),
            "raw_input_rate": self.num("demuxer-cache-state/raw-input-rate"),
            "height": int(self.num("video-params/h")),
            "codec": self.get("video-codec"),
            "hwdec_current": self.get("hwdec-current"),
            "frames": self.frames - first,
        }
        assert seen["time_pos"] > seconds, f"stuck at {seen['time_pos']:.2f}s"
        assert seen["frames"] > 0, "the render API gave no frame"
        return seen

    def save_png(self, path):
        with self.lock:
            raw = ctypes.string_at(ctypes.addressof(self.pixels) + self.offset, STRIDE * H)
        rows = b"".join(b"\0" + bytes(c for i, c in enumerate(raw[y * STRIDE:(y + 1) * STRIDE]) if i % 4 != 3) for y in range(H))
        lit = sum(1 for i in range(0, len(raw), 4 * 97) if raw[i] | raw[i + 1] | raw[i + 2])

        def chunk(kind, data):
            return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

        with open(path, "wb") as f:
            f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", W, H, 8, 2, 0, 0, 0))
                    + chunk(b"IDAT", zlib.compress(rows, 6)) + chunk(b"IEND", b""))
        return lit

    def close(self):
        self.stop.set()
        self.thread.join()
        # The render context goes first: render.h requires it freed before the core is destroyed.
        self.lib.mpv_render_context_free(self.ctx)
        self.lib.mpv_terminate_destroy(self.h)


def main():
    lib_path, out = sys.argv[1], sys.argv[2]
    os.makedirs(out, exist_ok=True)
    arch = platform.machine()
    lib = load(lib_path)
    v = lib.mpv_client_api_version()
    print(f"{arch}: libmpv client API {v >> 16}.{v & 0xFFFF}")
    p = Player(lib, os.path.join(out, f"mpv-{arch}.log"))
    results, ok = {}, True
    try:
        for name, url in STREAMS:
            try:
                seen = p.play(url, "max")
                seen["lit_samples"] = p.save_png(os.path.join(out, f"{name}-{arch}.png"))
                assert seen["raw_input_rate"] > 0, "no input rate"
                assert seen["lit_samples"] > 0, "the frame is all black"
                results[name] = seen
                print(f"{name}: ok {json.dumps(seen)}")
            except AssertionError as e:
                ok = False
                results[name] = {"error": str(e)}
                print(f"{name}: FAILED {e}")
        # hls-bitrate picks a variant: the lowest must be smaller than the highest.
        try:
            high = results.get("mux-h264", {}).get("height", 0)
            low = p.play(STREAMS[1][1], "min")["height"]
            assert 0 < low < high, f"hls-bitrate=min gave {low}p, max gave {high}p"
            results["hls-bitrate"] = {"min": low, "max": high}
            print(f"hls-bitrate: ok min {low}p < max {high}p")
        except AssertionError as e:
            ok = False
            results["hls-bitrate"] = {"error": str(e)}
            print(f"hls-bitrate: FAILED {e}")
    finally:
        p.close()
    with open(os.path.join(out, f"smoke-{arch}.json"), "w") as f:
        json.dump(results, f, indent=2)
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
