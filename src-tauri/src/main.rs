// Без консоли в release: браузер — не консольная утилита.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    browser190x4::run()
}
