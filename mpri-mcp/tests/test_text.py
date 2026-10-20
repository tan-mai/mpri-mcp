import pytest

from mpri_mcp.utils.text import (
    fuzzy_rank,
    normalize_base_url,
    normalize_name,
    same_site,
    slugify,
    strip_diacritics,
)


def test_strip_diacritics_handles_d_with_stroke():
    assert strip_diacritics("Đường đi Sữa") == "Duong di Sua"


def test_normalize_name():
    assert (
        normalize_name("Thùng Sữa TH True Milk  Ít Đường 110ml")
        == "thung sua th true milk it duong 110ml"
    )


def test_slugify():
    assert slugify("Lotte Mart") == "lotte-mart"
    assert slugify("Co.op Mart") == "co-op-mart"


@pytest.mark.parametrize(
    ("raw", "expected"),
    [
        ("lottemart.vn", "https://lottemart.vn"),
        ("https://WWW.Winmart.vn/", "https://www.winmart.vn"),
        ("http://example.com/shop/", "http://example.com/shop"),
    ],
)
def test_normalize_base_url(raw, expected):
    assert normalize_base_url(raw) == expected


@pytest.mark.parametrize("raw", ["", "ftp://x.vn", "localhost", "https://"])
def test_normalize_base_url_rejects(raw):
    with pytest.raises(ValueError):
        normalize_base_url(raw)


def test_same_site():
    assert same_site("https://online.mmvietnam.com/p/1.html", "https://mmvietnam.com")
    assert same_site("https://www.winmart.vn/x", "https://winmart.vn")
    assert not same_site("https://shopee.vn/x", "https://winmart.vn")


def test_fuzzy_rank_without_diacritics():
    candidates = [
        (1, "Thùng Sữa TH True Milk Ít Đường 110ml"),
        (2, "Nước tinh khiết Satori thùng 24 chai 500ml"),
        (3, "Nước tinh khiết Satori thùng 24 chai 350ml"),
    ]
    best = fuzzy_rank("sua th true milk it duong", candidates)[0]
    assert best.key == 1

    ranked = fuzzy_rank("satori 350ml", candidates)
    assert ranked[0].key == 3


def test_fuzzy_rank_exact_match_wins():
    candidates = [(1, "Satori 500ml"), (2, "Satori 500ml thùng 24 chai")]
    assert fuzzy_rank("Satori 500ml", candidates)[0].key == 1


def test_fuzzy_rank_no_match():
    assert fuzzy_rank("bia tiger", [(1, "Sữa TH True Milk")]) == []
