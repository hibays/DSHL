fn main() {
    // Emits per-platform linker args for the napi cdylib — most importantly
    // `-Wl,-undefined,dynamic_lookup` on macOS, whose strict ld otherwise
    // rejects the lazily-resolved NAPI symbols (`_napi_get_undefined`, …).
    napi_build::setup();
}
