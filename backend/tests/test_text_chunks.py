import pytest

from openreader_backend.utils.text import normalize_selection, split_long_text, split_sentences


@pytest.mark.parametrize(
    "text",
    [
        "Das kostet 3.14 Euro. Version 1.2.3 bleibt gleich!",
        "https://example.org/path?q=1.2 ist die URL. Danach geht es weiter.",
        'Wirklich?! Ja... Und dann: "Fertig!" Na gut.',
        "Herr Dr. Meier liest z. B. diesen Text. Ende.",
        "Hallo!!! Was??? ...",
        "你好！！ 下一句。 再见！",
    ],
)
def test_chunks_preserve_all_text(text):
    for limit in (30, 60, 420):
        chunks = split_sentences(text, limit)
        assert " ".join(chunks) == normalize_selection(text)
        assert all(0 < len(chunk) <= limit for chunk in chunks)


def test_long_unbroken_word_is_not_dropped():
    text = "x" * 1000
    assert "".join(split_sentences(text, 90)) == text


@pytest.mark.parametrize("splitter", [split_sentences, split_long_text])
def test_invalid_chunk_size_fails_clearly(splitter):
    with pytest.raises(ValueError, match="positive"):
        splitter("Text", 0)


def test_empty_text_has_no_chunks():
    assert split_sentences(" \n ") == []
