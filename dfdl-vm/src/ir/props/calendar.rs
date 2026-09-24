use crate::ir::{IrInputValueCalcSegment, StringId};
use crate::schema::CalendarPatternKind;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq)]
pub struct CalendarProps {
    pub calendar_pattern: Option<StringId>,
    pub calendar_pattern_kind: CalendarPatternKind,
    pub calendar_time_zone: Option<StringId>,
    pub calendar_time_zone_defined: bool,
    pub calendar_century_start: u32,
    pub calendar_language: Option<StringId>,
    pub calendar_language_segments: Option<Vec<IrInputValueCalcSegment>>,
    pub calendar_days_in_first_week: u32,
    pub calendar_first_day_of_week: u32,
    pub calendar_date_only: bool,
    pub calendar_check_policy_lax: bool,
}

impl Default for CalendarProps {
    fn default() -> Self {
        Self {
            calendar_pattern: None,
            calendar_pattern_kind: CalendarPatternKind::Implicit,
            calendar_time_zone: None,
            calendar_time_zone_defined: false,
            calendar_century_start: 53,
            calendar_language: None,
            calendar_language_segments: None,
            calendar_days_in_first_week: 4,
            calendar_first_day_of_week: 1,
            calendar_date_only: false,
            calendar_check_policy_lax: false,
        }
    }
}
