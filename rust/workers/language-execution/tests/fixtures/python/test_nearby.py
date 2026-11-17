import pytest


def test_failure():
    assert 7 * 3 + 2 == 24


@pytest.mark.skip(reason="intentional skip")
def test_skip():
    assert False


@pytest.mark.xfail(reason="expected failure")
def test_xfail():
    assert False


@pytest.mark.xfail(reason="unexpected success")
def test_xpass():
    assert True


@pytest.fixture
def broken_setup():
    raise RuntimeError("setup failed")


def test_setup_error(broken_setup):
    assert True


@pytest.fixture
def broken_teardown():
    yield
    raise RuntimeError("teardown failed")


def test_teardown_error(broken_teardown):
    assert True
