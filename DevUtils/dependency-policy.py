#!/usr/bin/env python3
"""Enforce Slate's compatible dependency requirement policy."""
import pathlib
import re
import tomllib

root = pathlib.Path(__file__).resolve().parent.parent
for manifest in root.rglob('Cargo.toml'):
    if 'target' in manifest.parts:
        continue
    data = tomllib.loads(manifest.read_text())
    def check(value):
        if isinstance(value, dict):
            for key, entry in value.items():
                if key.endswith('dependencies') and isinstance(entry, dict):
                    for name, requirement in entry.items():
                        version = requirement if isinstance(requirement, str) else requirement.get('version', '')
                        assert not any(part.strip().startswith('=') for part in version.split(',')), f'{manifest}: exact dependency {name}'
                check(entry)
    check(data)
cmake = (root/'crates/slate-gui/native/CMakeLists.txt').read_text()
assert not re.search(r'find_package\([^)]*\bEXACT\b', cmake), 'Exact native runtime requirement'
# Audio playback is optional: never link Qt Multimedia or require it to build.
assert 'Multimedia' not in cmake, 'Qt Multimedia must stay an optional runtime feature'
for qml in (root/'crates/slate-gui/native').glob('*.qml'):
    text = qml.read_text()
    assert not re.search(r'^import\s+[\w.]+\s+\d', text, re.M), f'Versioned QML import in {qml}'
    # Only Qt's own modules: the GUI must run on any desktop with Qt 6.
    assert not re.search(r'^import\s+org\.kde', text, re.M), f'KDE-only QML import in {qml}'
    # One lazily loaded file owns the optional module, so its absence cannot
    # break the window.
    if re.search(r'^import\s+QtMultimedia', text, re.M):
        assert qml.name == 'AudioPlayer.qml', f'QtMultimedia imported outside AudioPlayer.qml: {qml}'
print('PASS dependency policy: compatible Cargo requirements, minimum Qt API, versionless QML, no KDE-only modules, optional multimedia')
