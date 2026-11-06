# Nix Launcher

A minimalist black and white Minecraft launcher for Linux. Written in Rust.

It downloads the game, the assets and a matching Java runtime for you, then starts Minecraft with your nickname. No clutter, no web view, one small native binary.

> Status: early. Offline mode only (nickname). Microsoft account sign-in is on the roadmap.

## Install

### One line (Ubuntu, Debian and most other distros)

```bash
curl -fsSL https://raw.githubusercontent.com/Bobistuca/-nix-Launchear/main/install.sh | sh
```

On Ubuntu and Debian this installs the `.deb` package. On other distros it installs the AppImage into `~/.local/bin`. To force the AppImage everywhere:

```bash
curl -fsSL https://raw.githubusercontent.com/Bobistuca/-nix-Launchear/main/install.sh | NIX_APPIMAGE=1 sh
```

### Manual downloads

Grab a file from the [latest release](https://github.com/Bobistuca/-nix-Launchear/releases/latest):

| Format | File | Notes |
| --- | --- | --- |
| Debian / Ubuntu | `nix-launcher_amd64.deb` | `sudo apt install ./nix-launcher_amd64.deb` |
| Any Linux | `nix-launcher-x86_64.AppImage` | `chmod +x` it and run it |
| Any Linux | `nix-launcher-x86_64.tar.gz` | Plain binary |

Then run `nix-launcher` or open it from your application menu.

### Build from source

```bash
sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev libgl1-mesa-dev libegl1-mesa-dev
git clone https://github.com/Bobistuca/-nix-Launchear.git
cd -nix-Launchear
cargo run --release
```

## How it works

1. Pick a nickname and a version, then press Play.
2. The launcher fetches the official version manifest from Mojang and downloads the client, libraries and assets, verifying checksums.
3. It looks for a suitable Java on your system. If none is found, it downloads a JRE from Adoptium into its own data folder.
4. Minecraft is started and its output goes to `~/.local/share/nix-launcher/logs/latest.log`.

Data lives in `~/.local/share/nix-launcher`. Settings live in `~/.config/nix-launcher/config.json`.

## Roadmap

- Microsoft account sign-in for the original game
- Fabric and Forge profiles
- Multiple instances
- Mod and resource pack management

## License

MIT. See [LICENSE](LICENSE). Nix Launcher is not affiliated with Mojang or Microsoft. Minecraft is a trademark of Mojang Synergies AB.
