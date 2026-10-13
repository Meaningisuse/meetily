# Audio test fixtures

`jfk.wav` and `jfk.mp3` were copied from `samples/` in the historical
[Zackriya-Solutions/whisper.cpp](https://github.com/Zackriya-Solutions/whisper.cpp)
submodule at commit `d682e150908e10caa4c15883c633d7902d385237`.
The upstream MIT license is preserved in `WHISPER_CPP_LICENSE.txt`.
They support audio import metadata and validation tests without the archived backend.

`he_aac_48k_5s.m4a` is the existing HE-AAC decoder regression fixture.

Resolve fixtures from `env!("CARGO_MANIFEST_DIR")/tests/fixtures`, independently
of the shell working directory. A missing required fixture must fail its test.
