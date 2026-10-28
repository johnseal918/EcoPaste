import ctypes as c
from ctypes import wintypes as w
from datetime import datetime, timezone
import json
from pathlib import Path
import threading

u = c.WinDLL('user32', use_last_error=True)
u.OpenClipboard.argtypes = [w.HWND]
u.EnumClipboardFormats.argtypes = [w.UINT]
result = {'at_utc': datetime.now(timezone.utc).isoformat(), 'scope': 'native API mechanism only; not an application trace'}
before = u.GetClipboardSequenceNumber()
result['initial_open'] = bool(u.OpenClipboard(None))
if result['initial_open']:
    try:
        result['nested_open'] = bool(u.OpenClipboard(None))
        if result['nested_open']:
            u.CloseClipboard()
        c.set_last_error(0)
        value = u.EnumClipboardFormats(0)
        result['enum_error_after_nested_close'] = c.get_last_error() if not value else 0
    finally:
        u.CloseClipboard()
    result['cross_thread_initial_open'] = bool(u.OpenClipboard(None))
    if result['cross_thread_initial_open']:
        try:
            def competing_thread():
                result['second_thread_open'] = bool(u.OpenClipboard(None))
                if result['second_thread_open']:
                    u.CloseClipboard()
            worker = threading.Thread(target=competing_thread)
            worker.start()
            worker.join()
            c.set_last_error(0)
            value = u.EnumClipboardFormats(0)
            result['first_thread_enum_error_after_second_close'] = c.get_last_error() if not value else 0
        finally:
            u.CloseClipboard()
else:
    result['initial_error'] = c.get_last_error()
result['clipboard_sequence_unchanged'] = before == u.GetClipboardSequenceNumber()
(Path(__file__).parent / 'native-lock-probe.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
print(json.dumps(result))
