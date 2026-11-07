# What yakin draws from Valve's Steam Controller shell CAD, made at build time so the repo holds
# none of it: the CAD and everything derived from it are CC BY-NC-SA 4.0 (see NOTICE)
{ pkgs }:
let
  release = "https://gitlab.steamos.cloud/SteamHardware/SteamController/-/raw/e10919b4b86249817aaca1160aa136ef5622c533";
in
rec {
  stl = pkgs.fetchurl {
    url = "${release}/sc_solid_stl_20260429.stl";
    hash = "sha256-xPSF6nSeJ2GbOr9T7hgfRVHLj0Ur4KXZAJERPrc7ebU=";
  };

  license = pkgs.fetchurl {
    url = "${release}/license.txt";
    hash = "sha256-GbIl4cpgXXFF0p5Yxn2R+JFrxjy1mFGNBfmKyP/f0gA=";
  };

  # The licence and attribution that go next to every derived file
  legal = pkgs.runCommand "yakin-cad-legal" { } ''
    install -Dm644 ${license} $out/LICENSE
    install -Dm644 ${../NOTICE} $out/NOTICE
  '';

  # controller.svg, icon.svg, icon-symbolic.svg and the icon's PNGs (icons/<size>.png)
  drawing =
    pkgs.runCommand "yakin-drawing"
      {
        nativeBuildInputs = [
          (pkgs.python3.withPackages (p: [
            p.numpy
            p.scipy
            p.scikit-image
          ]))
          pkgs.resvg
        ];
      }
      ''
        python -I ${./drawing.py} ${stl} $out
        mkdir $out/icons
        for size in 32 128 256 512; do
          resvg -w $size -h $size $out/icon.svg $out/icons/$size.png
        done
        cp ${legal}/* $out/
      '';

  # The 3D view's shell: the STL decimated in Blender to binary glTF
  model =
    pkgs.runCommand "yakin-controller.glb" { nativeBuildInputs = [ pkgs.blender ]; }
      ''
        HOME=$TMPDIR blender -b --python ${./convert.py} -- ${stl} $TMPDIR/controller.glb 0.04
        mv $TMPDIR/controller.glb $out
      '';
}
