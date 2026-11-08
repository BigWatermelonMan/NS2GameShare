"""The public source boundary, shared by exporter and audit. No wildcard trees."""
from pathlib import PurePosixPath

ROOT_FILES = {
    'README.md', 'README.ja.md', 'LICENSE', '.gitignore', '.gitattributes', 'SECURITY.md',
    'THIRD_PARTY_NOTICES.md', 'PUBLIC_MANIFEST.json',
    '.github/workflows/check.yml',
    'docs/INPUT.md', 'docs/PASSPHRASE.md', 'docs/BUILD.md', 'docs/RELEASING.md', 'docs/THIRD_PARTY.json',
    'docs/PROTOCOL.md', 'docs/DEVELOPMENT.md',
    'docs/PROTOCOL.ja.md', 'docs/DEVELOPMENT.ja.md',
    'client-windows/Cargo.toml', 'client-windows/Cargo.lock',
    'client-windows/README.md', 'client-windows/input.example.json',
    'client-windows/run.ps1',
    'firmware/CMakeLists.txt', 'firmware/sdkconfig.defaults',
    'firmware/main/main.c', 'firmware/main/CMakeLists.txt', 'firmware/main/Kconfig.projbuild',
    'firmware-basic/CMakeLists.txt', 'firmware-basic/sdkconfig.defaults',
    'firmware-basic/build.ps1', 'firmware-basic/README.md',
    'firmware-basic/main/main.c', 'firmware-basic/main/fx2_fifo.c',
    'firmware-basic/main/fx2_fifo.h', 'firmware-basic/main/CMakeLists.txt',
    'firmware-basic/main/Kconfig.projbuild',
    'firmware-fx2/main.c', 'firmware-fx2/build.ps1', 'firmware-fx2/README.md',
    'firmware-fx2/return_uart.inc', 'firmware-fx2/winusb_properties.inc',
}


def allowed(name):
    path = PurePosixPath(name)
    if '..' in path.parts or path.is_absolute():
        return False
    return (name in ROOT_FILES
            or (name.startswith('client-windows/src/') and path.suffix == '.rs')
            or (name.startswith('tools/release/') and path.suffix == '.py')
            or (name.startswith('licenses/third-party/') and path.suffix == '.txt'))
