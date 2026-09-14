//! Сетевой фильтр браузера 190x4.
//!
//! Контракт крейта: **`Guard::check` вызывается из COM-потока UI внутри
//! обработчика `WebResourceRequested` и обязан вернуться за единицы
//! микросекунд.** Всё, что может занять больше, — парсинг списков,
//! перекомпиляция движка, сеть — живёт в [`update`] и работает на фоновом
//! потоке; готовый движок въезжает в горячий путь одним `ArcSwap::store`.
//!
//! Отсюда два инварианта, которые легко сломать при доработке:
//! 1. в нашем `check` нет своих локов — только `ArcSwap::load` (RCU-чтение);
//!    единственный лок на пути — `Mutex` regex-кэша внутри adblock, и он не
//!    контендится, потому что читатель один (UI-поток);
//! 2. в `check` нет ни одной аллокации сверх той, что делает `Request::new`.

mod cosmetic;
mod engine;
mod lists;
mod stats;

pub use cosmetic::{document_host, document_script, Cosmetics};
pub use engine::{site_key, Decision, FilterList, Guard, ResourceKind};
pub use lists::{ListSource, ListSpec, Subscriptions};
pub use stats::{Snapshot, Stats};
