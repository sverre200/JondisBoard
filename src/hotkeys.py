"""Global hotkeys that work while a game has focus.

Bindings match on scan code + numpad flag, so Numpad 1 and the normal 1 key
are different hotkeys (the `keyboard` library's name-based hotkeys can't tell them apart)."""
import threading

import keyboard

MODIFIERS = ("ctrl", "shift", "alt")


def binding_label(b):
    if not b:
        return ""
    return "+".join(list(b.get("mods", [])) + [b.get("label", "?")])


def _binding_key(b):
    return (b["scan"], bool(b.get("keypad")), tuple(sorted(b.get("mods", []))))


class HotkeyManager:
    def __init__(self):
        self._actions = {}        # binding key -> callable
        self._down = set()        # scan codes currently held (ignores key autorepeat)
        self._recorder = None
        self._lock = threading.Lock()
        keyboard.hook(self._on_event)

    def set_bindings(self, pairs):
        """pairs: iterable of (binding dict, callable)."""
        with self._lock:
            self._actions = {_binding_key(b): fn for b, fn in pairs if b}

    def record(self, callback):
        """Capture the next key press (with modifiers); callback(binding or None if Esc)."""
        with self._lock:
            self._recorder = callback

    def cancel_record(self):
        with self._lock:
            self._recorder = None

    def _on_event(self, e):
        if e.scan_code is None:
            return
        if e.event_type == keyboard.KEY_UP:
            self._down.discard(e.scan_code)
            return
        if e.scan_code in self._down:
            return
        self._down.add(e.scan_code)
        if keyboard.is_modifier(e.name) or keyboard.is_modifier(e.scan_code):
            return

        mods = [m for m in MODIFIERS if keyboard.is_pressed(m)]
        keypad = bool(getattr(e, "is_keypad", False))
        name = (e.name or f"key{e.scan_code}")
        binding = {"scan": e.scan_code, "keypad": keypad, "mods": mods,
                   "label": ("num " + name) if keypad else name}

        with self._lock:
            recorder, self._recorder = self._recorder, None
            action = None if recorder else self._actions.get(_binding_key(binding))
        if recorder:
            recorder(None if name == "esc" else binding)
        elif action:
            try:
                action()
            except Exception:
                pass

    def close(self):
        keyboard.unhook_all()
