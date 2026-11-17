#!/usr/bin/env python3
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check upgrade migration and resource fingerprints using temporary files."""
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

class StorageTest(unittest.TestCase):
    def test_migration_preserves_data_and_stops_on_collision(self):
        patch = (ROOT / 'build-aux/android/storage-preservation.patch').read_text()
        added = '\n'.join(line[1:] for line in patch.splitlines() if line.startswith('+') and not line.startswith('+++'))
        method = added[added.index('\t// Hopea: move legacy'):]
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / 'Migration.java'
            source.write_text('''import java.io.*;
import java.nio.file.*;
class Migration {
    File legacy;
    Migration(File legacy) { this.legacy = legacy; }
    File getFilesDir() { return legacy; }
    Migration getAssets() { return this; }
    String[] list(String path) { return new String[] {"share", "etc", "afpr"}; }
''' + method + '''
    public static void main(String[] args) throws Exception {
        Path root = Paths.get(args[0]);
        Path legacy = Files.createDirectory(root.resolve("legacy"));
        Path home = Files.createDirectory(root.resolve("home"));
        for (String file : new String[] {".config/hopea/settings", ".local/share/hopea/library.db", "Music/song.flac", "notes.txt", "share/runtime", "etc/fonts.conf", "afpr"}) {
            Path p = legacy.resolve(file); Files.createDirectories(p.getParent()); Files.writeString(p, file);
        }
        Migration migration = new Migration(legacy.toFile());
        migration.hopeaPreserveUserFiles(home.toFile());
        migration.hopeaPreserveUserFiles(home.toFile()); // Second launch is a no-op.
        for (String file : new String[] {".config/hopea/settings", ".local/share/hopea/library.db", "Music/song.flac", "notes.txt"}) {
            assert Files.readString(home.resolve(file)).equals(file);
            assert !Files.exists(legacy.resolve(file));
        }
        for (String file : new String[] {"share/runtime", "etc/fonts.conf", "afpr"}) {
            assert Files.exists(legacy.resolve(file));
            assert !Files.exists(home.resolve(file));
        }
        Files.writeString(legacy.resolve("notes.txt"), "legacy-conflict");
        try { migration.hopeaPreserveUserFiles(home.toFile()); throw new AssertionError("Must stop before cleanup"); }
        catch (IOException expected) {}
        assert Files.readString(legacy.resolve("notes.txt")).equals("legacy-conflict");
        assert Files.readString(home.resolve("notes.txt")).equals("notes.txt");
    }
}
''')
            subprocess.run(['javac', '-d', str(root), str(source)], check=True)
            subprocess.run(['java', '-ea', '-cp', str(root), 'Migration', str(root)], check=True)

    def test_fingerprint_includes_packaged_fonts_and_paths(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            (root / 'font.ttf').write_bytes(b'font fixture')
            script = ROOT / 'build-aux/android/fingerprint-assets.py'
            def update():
                subprocess.run([sys.executable, str(script), str(root)], check=True)
                return (root / 'afpr').read_bytes()
            first = update()
            self.assertEqual(len(first), 128)
            self.assertEqual(update(), first)
            (root / 'font.ttf').write_bytes(b'updated font fixture')
            second = update()
            self.assertNotEqual(second, first)
            (root / 'font.ttf').rename(root / 'renamed.ttf')
            self.assertNotEqual(update(), second)

if __name__ == '__main__':
    unittest.main()
