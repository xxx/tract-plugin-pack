# Publish a plugin release

Record user-visible changes as they land under **Unreleased** and name the
affected plugins or formats. [Changelog](../../CHANGELOG.md)

1. In the repository's **Settings → General → Releases**, turn on **Enable
   release immutability** before publishing. [GitHub's setup instructions](https://docs.github.com/en/code-security/how-tos/secure-your-supply-chain/establish-provenance-and-integrity/prevent-release-changes)
2. Create an active tag ruleset matching `v[0-9]*` with **Restrict updates**
   and **Restrict deletions** enabled and an empty bypass list. This keeps the
   tag fixed while the draft is under review. [GitHub's ruleset instructions](https://docs.github.com/en/repositories/configuring-branches-and-merges-in-your-repository/managing-rulesets/creating-rulesets-for-a-repository)
3. Move the **Unreleased** entries into a versioned changelog section headed
   `## 0.1.0 - YYYY-MM-DD`, using the release version and date. Keep an empty
   **Unreleased** section at the top for future changes. Include the changelog
   update in the release commit, then tag the commit and push the tag.

   ```bash
   git tag -a v0.1.0 -m "Release v0.1.0"
   git push origin refs/tags/v0.1.0
   ```

4. After **Release Plugins** succeeds, open the draft under **Releases**. Use
   the matching changelog section for the release notes and keep the
   **Built from commit** line and macOS installation link. Confirm that all 14
   ZIPs are attached. Immediately before publishing, resolve the remote tag
   and compare the result to the full commit SHA on the **Built from commit**
   line. Run this from the repository root and substitute your release tag.

   ```bash
   gh api --method GET 'repos/{owner}/{repo}/commits' \
     -f 'sha=refs/tags/v0.1.0' -F per_page=1 --jq '.[0].sha'
   ```

5. If the SHAs match, mark prereleases as needed and select **Publish release**.
   GitHub locks the tag and assets at publication. If the SHAs differ or the
   lookup fails, leave the draft unpublished and resolve the discrepancy first.
   [GitHub's release instructions](https://docs.github.com/en/repositories/releasing-projects-on-github/managing-releases-in-a-repository)
6. Check for GitHub's **Immutable** badge or run `gh release verify v0.1.0`.

To build preview ZIPs, run **Release Plugins** manually from **Actions** and
download them from the workflow run's **Artifacts** section.

## Release ZIPs

| Platform | Redistributable | Native |
|---|---|---|
| Linux | x86-64 | x86-64 |
| Windows | x86-64 | x86-64 |
| macOS | Universal (Intel and Apple Silicon) | Intel and Apple Silicon downloads |

Every build has one CLAP ZIP and one VST3 ZIP. Each ZIP includes the full plugin
collection in that format, the project and third-party notices, and the
illustrated manuals and MSEG guide in PDF and Markdown under `docs/`.

macOS ZIPs are labeled `unnotarized` and include the steps for approving a
plug-in if Gatekeeper blocks it. [macOS installation guide](../installing-macos.md)

Redistributable builds use Rust's generic target CPU. Native builds use the
[bundler's CPU detection](../../README.md#host-optimized-builds), which targets
Haswell on x86-64 runners with AVX2, FMA and BMI2 and uses the generic target CPU
on Apple Silicon.
