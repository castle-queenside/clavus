"""Test: does Static.update() work from inside a @work worker?"""
import asyncio
from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.widgets import Static


class TestApp(App):
    BINDINGS = [Binding("u", "update_header", "Update")]

    def compose(self) -> ComposeResult:
        yield Static("initial text", id="header")

    def on_mount(self):
        self._count = 0

    @work
    async def action_update_header(self):
        self._count += 1
        widget = self.query_one("#header", Static)
        widget.update(f"updated {self._count}")
        self.refresh()


if __name__ == "__main__":
    app = TestApp()
    app.run()
