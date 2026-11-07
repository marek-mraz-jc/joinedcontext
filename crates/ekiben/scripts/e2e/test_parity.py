"""Checks of the comparison in parity.py: `python3 scripts/e2e/test_parity.py` (or pytest)."""
import parity


def test_a_now_time_as_far_from_its_runs_start_is_the_same():
    bento = '{"at":"2026-11-07T08:00:05Z","n":1}'
    ekiben = '{"at":"2026-11-09T10:00:07Z","n":1}'
    shift = (2 * 86400) + 7200
    assert parity.align(bento, ekiben, shift) == bento


def test_a_time_further_off_or_any_other_byte_still_differs():
    bento = '{"at":"2026-11-07T08:00:05Z","n":1}'
    assert parity.align(bento, '{"at":"2026-11-07T08:01:05Z","n":1}', 0) != bento
    assert parity.align(bento, '{"at":"2026-11-07T08:00:05Z","n":2}', 0) != bento


def test_epoch_seconds_and_millis_align_too():
    assert parity.align('{"t":1762502400}', '{"t":1762502403}', 3) == '{"t":1762502400}'
    assert parity.align('{"t":1762502400000}', '{"t":1762502403000}', 3) == '{"t":1762502400000}'


def test_the_first_difference_names_the_field_and_place():
    run = {"exit": "ok", "messages": ['{"content":"a"}'], "requests": [], "started": 0}
    other = dict(run, messages=['{"content":"b"}'])
    assert parity.first_difference(run, run) is None
    assert parity.first_difference(run, other).startswith("messages[0] at char 12")


if __name__ == "__main__":
    for name, test in list(globals().items()):
        if name.startswith("test_"):
            test()
    print("parity comparison checks passed")
