from typing import Protocol

class SpelingProvider(Protocol):
    def candiate(self, word: str) -> list[str]: ...
