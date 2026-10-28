# The .NET SDK leg (lane TSDK-dotnet, D2140-D2142), sourced by run.sh when
# SDK_LEGS names `dotnet` (D2138). Temporalio (pins.env TEMPORAL_SDK_DOTNET,
# corpus/workers/dotnet/Corpus.csproj) on net8.0 in the pinned DOTNET_IMAGE:
# the worker and the starter run the scenarios against the gateway, the Go
# exporter writes the histories (as for TypeScript and Python), and the .NET
# WorkflowReplayer checks them. Uses run.sh's leg, go_run, $rt, $addr,
# ${net[@]}, $corpus, $out, $SCENARIOS. Not yet run (honest status: CI's
# first run).
# shellcheck shell=bash

dotnet_env=(-e DOTNET_CLI_TELEMETRY_OPTOUT=1 -e DOTNET_NOLOGO=1 -e DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1)

# Builds the worker once into $out/dotnet-bin (the replayer reuses it), runs
# the scenarios; the gateway serves no OperatorService before DX4, so no
# custom search attribute.
leg_dotnet_generate() {
  "$rt" volume create dx1-nugetcache >/dev/null 2>&1 || true
  rm -rf "$out/dotnet-bin"
  leg dotnet-generate "${net[@]}" -v "$corpus:/c:ro" -v "$out:/out" -v dx1-nugetcache:/root/.nuget/packages \
    "${dotnet_env[@]}" "$DOTNET_IMAGE" \
    sh -ec "cp -r /c/workers/dotnet /tmp/d && cd /tmp/d && dotnet build -c Release -o /out/dotnet-bin -v quiet -nologo &&
      dotnet /out/dotnet-bin/corpus-dotnet.dll generate $addr default /out/generate-dotnet.json $SCENARIOS --no-search-attribute" &&
    go_run dotnet-export "workers/go && go build -o /tmp/corpus . && /tmp/corpus export -address $addr -namespace default -no-search-attribute -prefix corpus-dotnet- -scenarios $SCENARIOS -out /out/histories/dotnet"
}

# The replayer never dials the server: no network.
leg_dotnet_replay() {
  [ -s "$out/dotnet-bin/corpus-dotnet.dll" ] || { echo "the .NET worker did not build"; return 1; }
  leg dotnet-replay --network none -v "$out:/out:ro" "${dotnet_env[@]}" "$DOTNET_IMAGE" \
    dotnet /out/dotnet-bin/corpus-dotnet.dll replay /out/histories/dotnet
}
