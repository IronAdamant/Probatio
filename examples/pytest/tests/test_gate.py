from gate import gate


def test_zero_stays_shut():
    assert gate(0) is False
