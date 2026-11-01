# angel-ear

Hears the kick, snare, hi-hats and loudness of what a program plays, as it happens, for
[angelOS](https://github.com/MixaDoDs/AngelOS-Dotfiles). No dependencies beyond `pactl` and `parec`.

    angel-ear             listens to the playing program; prints K|S|H <strength>, B, F <bass> <loud>
    angel-ear --file F    the same from raw mono f32 at 48 kHz, with the time first
    cargo test --release  a made-up song with known hits
