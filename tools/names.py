"""Fills names.txt, the bot names `run` takes its characters from, up to a count of names.

The names already in the file stay first, in their order: a bot keeps its character from one run to the next. The new
names are made of syllables, from a fixed seed, so the same count gives the same file.

    python tools/names.py [count]     # default 640
"""

import random
import sys
from pathlib import Path

PATH = Path(__file__).resolve().parent.parent / "names.txt"

ONSETS = ["b", "br", "c", "d", "dr", "f", "g", "gr", "h", "j", "k", "kh", "l", "m", "n", "p", "r", "s", "sh", "st",
          "t", "th", "tr", "v", "w", "y", "z", "al", "el", "or", "is", "ul", "ar", "ev", "od", "ir"]
VOWELS = ["a", "e", "i", "o", "u", "ae", "ia", "ei", "y"]
CODAS = ["", "", "", "n", "l", "r", "s", "th", "nd", "rn", "ric", "wyn", "mir", "dor", "len", "ra", "na", "lia",
         "ven", "mar", "rin", "ssa", "da", "nor", "gar", "wen", "dric", "vin", "ros", "tha"]
# Parts no bot should be named with, in any language a player may read them in
BLOCKED = ["ass", "fag", "nig", "cum", "sex", "tit", "fuk", "fuc", "shit", "dick", "cock", "rape", "porn", "anal",
           "nazi", "hitl", "jew", "gay", "cunt", "slut", "whor", "piss", "poo", "pee", "turd", "butt", "damn", "hell",
           "god", "satan", "isis", "puss", "wank", "twat", "kike", "spic", "chink", "gook", "coon", "homo", "lesb",
           "dum", "nob", "kill", "dead", "die", "gm", "admin", "bot", "liar"]
VOWEL_LETTERS = set("aeiouy")


def name(rng: random.Random) -> str:
    parts = [rng.choice(ONSETS), rng.choice(VOWELS)]
    if rng.random() < 0.5:
        parts += [rng.choice(ONSETS[:26]), rng.choice(VOWELS)]
    parts.append(rng.choice(CODAS))
    return "".join(parts).capitalize()


def fine(n: str) -> bool:
    low = n.lower()
    if not 4 <= len(n) <= 8 or any(b in low for b in BLOCKED):
        return False
    # One vowel pair at most, and never a doubled vowel
    pairs = sum(1 for a, b in zip(low, low[1:]) if a in VOWEL_LETTERS and b in VOWEL_LETTERS)
    if pairs > 1 or any(a == b and a in VOWEL_LETTERS for a, b in zip(low, low[1:])):
        return False
    # No three of a kind in a row, no run of three vowels or four consonants
    run_v = run_c = 0
    for i, c in enumerate(low):
        if i >= 2 and c == low[i - 1] == low[i - 2]:
            return False
        run_v, run_c = (run_v + 1, 0) if c in VOWEL_LETTERS else (0, run_c + 1)
        if run_v >= 3 or run_c >= 4:
            return False
    return True


def main() -> None:
    count = int(sys.argv[1]) if len(sys.argv) > 1 else 640
    names = [n.strip() for n in PATH.read_text(encoding="utf-8").splitlines()] if PATH.exists() else []
    names = [n for n in names if n]
    seen = {n.lower() for n in names}
    rng = random.Random(1667)
    while len(names) < count:
        n = name(rng)
        if fine(n) and n.lower() not in seen:
            seen.add(n.lower())
            names.append(n)
    PATH.write_text("".join(f"{n}\n" for n in names), encoding="utf-8", newline="\n")
    print(f"{len(names)} names in {PATH}")


if __name__ == "__main__":
    main()
