# Portable Linux .NET 10 toolchain

The [GitHub toolchain release](https://github.com/Ryd-rb/SlopGuard/releases/tag/dotnet-linux-x64-10.0.404-r1) contains the original Microsoft Linux x64 SDK archive and the NuGet archives required by the solution locks at this release's tag. Keep both assets outside the checkout. They support a rootless install on glibc-based Linux x64 with the [.NET native dependencies](https://learn.microsoft.com/en-us/dotnet/core/install/linux#dependencies), Git, tar and sha256sum. Retrieve both archives before running the offline setup.

## Retrieve with the GitHub connector

Use the GitHub connector's **Fetch** action to list [successful workflow runs on main](https://api.github.com/repos/Ryd-rb/SlopGuard/actions/runs?event=workflow_dispatch&branch=main&status=success&per_page=10). Choose the newest run for `.github/workflows/portable-linux-toolchain.yml` and note its numeric run ID. Call **Fetch workflow run artifacts** with that run ID and `repo_full_name = Ryd-rb/SlopGuard`. Use the numeric IDs of the unexpired `slopguard-dotnet-sdk-10.0.404-linux-x64-r1` and `slopguard-nuget-feed-10.0.404-r1` assets. If there are more runs, page the listing.

Call the GitHub connector's **Download workflow artifact** action with `repo_full_name = Ryd-rb/SlopGuard` and each numeric artifact ID. Save the returned ZIP file references into the VM using its attachment or file tools. This transfer uses the GitHub connection. The VM shell can stay offline. Use `unzip` or an equivalent local ZIP reader to extract the two files into a fresh directory.

```sh
set -eu
tools_dir="$(mktemp -d)"
unzip -j /path/to/downloaded-sdk-artifact.zip 'dotnet-sdk-10.0.404-linux-x64.tar.gz' -d "$tools_dir"
unzip -j /path/to/downloaded-feed-artifact.zip 'slopguard-nuget-feed.tar.gz' -d "$tools_dir"
```

GitHub expires Actions artifacts. If either artifact has expired, the repository owner can run **Publish portable Linux toolchain** on `main`. The workflow verifies the two pinned release files before uploading fresh artifacts. [GitHub Actions artifact downloads](https://docs.github.com/en/rest/actions/artifacts#download-an-artifact)

## Retrieve with an online shell

```sh
set -eu
tools_dir="$(mktemp -d)"
release=https://github.com/Ryd-rb/SlopGuard/releases/download/dotnet-linux-x64-10.0.404-r1
sdk=dotnet-sdk-10.0.404-linux-x64.tar.gz
feed=slopguard-nuget-feed.tar.gz
curl --fail --location --retry 3 --output "$tools_dir/$sdk" "$release/$sdk"
curl --fail --location --retry 3 --output "$tools_dir/$feed" "$release/$feed"
```

## Verify, install and test offline

Run from the local SlopGuard checkout in the same shell that set `tools_dir`.

```sh
set -eu
sdk=dotnet-sdk-10.0.404-linux-x64.tar.gz
feed=slopguard-nuget-feed.tar.gz
(
  cd "$tools_dir"
  sha256sum -c <<'SHA256'
c0610757a818fde4b960d27a0ac421f273068a05dc5ec7abafc01dbf84e9cb4d  dotnet-sdk-10.0.404-linux-x64.tar.gz
8ecbe37f3ffb7df9433d7efad22b7631b2ee8f150dc94ad03a1bc46c840ae24c  slopguard-nuget-feed.tar.gz
SHA256
)
mkdir "$tools_dir/dotnet" "$tools_dir/feed" "$tools_dir/packages"
tar -xzf "$tools_dir/$sdk" -C "$tools_dir/dotnet"
tar -xzf "$tools_dir/$feed" -C "$tools_dir/feed"
export DOTNET_ROOT="$tools_dir/dotnet"
export PATH="$DOTNET_ROOT:$PATH"
export DOTNET_CLI_HOME="$tools_dir/cli"
export NUGET_PACKAGES="$tools_dir/packages"
export DOTNET_CLI_TELEMETRY_OPTOUT=1
test "$(dotnet --version)" = 10.0.404
dotnet restore SlopGuard.slnx --locked-mode --source "$tools_dir/feed" --no-http-cache -p:NuGetAudit=false
dotnet build SlopGuard.slnx --no-restore
dotnet test SlopGuard.slnx --no-build
```

The pinned SDK is for this Linux asset. The root SDK policy also accepts compatible .NET 10 feature bands for normal development. A VM with the Linux SDK can build and run the applicable tests. Linux supervision of repository code has separate [product prerequisites](architecture.md#worker-admission-and-recovery).

The local restore disables online vulnerability queries. Use the normal online restore and vulnerability checks when reviewing package changes. If a task changes package references, use an approved source to resolve them, include the updated lock files in the patch and report any validation blocked by restricted egress. A new release feed is needed when the locked package closure changes.

## Refresh an asset

Use the official [Microsoft .NET release metadata](https://github.com/dotnet/core/blob/main/release-notes/10.0/releases.json) to select a supported stable Linux x64 SDK and verify the upstream SHA-512. Assemble the feed from the union of external package IDs and resolved versions in the solution's lock files, keeping each original NuGet archive and its license notices intact. Verify with a fresh packages directory, locked restore from the staged feed, then build and test on Linux. Publish under a new release tag. Update the asset names, sizes and SHA-256 values in the C# staging tool, workflow and setup instructions. Run the workflow to publish connector artifacts for the new version. Keep the earlier tag and assets available to agents pinned to earlier commits. [NuGet locked restore](https://learn.microsoft.com/en-us/nuget/consume-packages/package-references-in-project-files#locking-dependencies)
