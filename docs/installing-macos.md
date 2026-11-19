# Install the plug-ins on macOS

1. Download the ZIP for the format your DAW supports. The universal build works
   on Intel and Apple Silicon. For native builds, choose the architecture your
   DAW runs on.
2. Before installing, you can confirm that the ZIP matches the immutable GitHub
   release. Replace `v0.1.0` and the filename with your download's tag and path.

   ```bash
   gh release verify-asset v0.1.0 /path/to/download.zip --repo xxx/tract-plugin-pack
   ```

3. Extract the ZIP. Copy the `.clap` bundles to `~/Library/Audio/Plug-Ins/CLAP/`
   or the `.vst3` bundles to `~/Library/Audio/Plug-Ins/VST3/`. Create the folder
   if needed.
4. Start your DAW and rescan for plug-ins.

## Approve a plug-in blocked by Gatekeeper

The macOS ZIPs are unnotarized. If Gatekeeper blocks a plug-in during the scan,
open **System Settings → Privacy & Security** and scroll to **Security**. If you
trust the download, choose **Open Anyway** for that plug-in and confirm. Rescan
or restart your DAW and repeat for each plug-in that macOS blocks.
[Apple's guidance for plug-ins](https://developer.apple.com/documentation/security/notarizing-macos-software-before-distribution)
