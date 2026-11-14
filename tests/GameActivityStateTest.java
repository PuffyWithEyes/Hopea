// SPDX-FileCopyrightText: 2026 Daniil Klimov
// SPDX-License-Identifier: GPL-3.0-or-later
package io.hopea.hopea;

public final class GameActivityStateTest {
    public static void main(String[] args) {
        GameActivityState state = new GameActivityState();
        assert state.begin(600_000) == 300_000;
        state.resume("game", "Main");
        assert "game".equals(state.foreground());
        state.resume("game", "Level");
        state.pause("game", "Main");
        assert "game".equals(state.foreground()) : "late activity pause";
        assert state.begin(605_000) == 598_000;
        assert "game".equals(state.foreground()) : "no new events while playing";
        state.resume("browser", "Main");
        state.pause("game", "Level");
        assert "browser".equals(state.foreground());
        state.pause("browser", "Main");
        assert state.foreground() == null;
        state.resume("game", "Main");
        state.begin(10); // clock moved backwards
        assert state.foreground() == null;
        state.resume("game", "Main");
        state.begin(600_000); // app suspended; old foreground state is unreliable
        assert state.foreground() == null;
        state.resume("game", "Main");
        state.reset(); // permission revoked / locked device
        assert state.foreground() == null;
        state.resume("x".repeat(513), "Main");
        assert state.foreground() == null;
        state.resume("game", null);
        assert state.foreground() == null;
        assert state.begin(Long.MIN_VALUE) == 0;
        assert state.foreground() == null;
        assert state.begin(Long.MAX_VALUE) == Long.MAX_VALUE - 300_000;
        System.out.println("Android game activity state: passed");
    }
}
