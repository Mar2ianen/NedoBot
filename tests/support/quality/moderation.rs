use super::generators::{ModerationOperation as Op, PrivilegeRole as Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptchaState {
    None,
    Pending,
    Passed,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restriction {
    Mute,
}

/// Independent policy model for the security boundaries of moderation and
/// CAPTCHA. It intentionally does not call production policy helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModerationModel {
    pub latest_not_spam: bool,
    pub source_current: bool,
    pub captcha: CaptchaState,
    pub captcha_owner: Option<u8>,
    pub captcha_owned_restriction: bool,
    pub manual_restriction: Option<Restriction>,
    pub automatic_effects: u32,
    pub manual_effects: u32,
    pub captcha_releases: u32,
    pub uncertain_delivery: bool,
    pub external_effects: u32,
    pub effects_at_uncertain_delivery: Option<u32>,
    pub duplicate_events: u32,
}

impl Default for ModerationModel {
    fn default() -> Self {
        Self {
            latest_not_spam: false,
            source_current: true,
            captcha: CaptchaState::None,
            captcha_owner: None,
            captcha_owned_restriction: false,
            manual_restriction: None,
            automatic_effects: 0,
            manual_effects: 0,
            captcha_releases: 0,
            uncertain_delivery: false,
            external_effects: 0,
            effects_at_uncertain_delivery: None,
            duplicate_events: 0,
        }
    }
}

impl ModerationModel {
    pub fn apply(&mut self, operation: Op) {
        match operation {
            Op::LabelNotSpam => self.latest_not_spam = true,
            Op::LabelSpam => self.latest_not_spam = false,
            Op::EditSource => self.source_current = false,
            Op::AutoEnforce { score }
                if self.source_current && !self.latest_not_spam && score >= 70 =>
            {
                self.automatic_effects += 1;
            }
            Op::ManualRestrict {
                actor,
                bot,
                target,
                target_is_bot,
            } if expected_manual_authorization(actor, bot, target, target_is_bot)
                && !self.uncertain_delivery
                && self.manual_restriction.is_none() =>
            {
                self.manual_restriction = Some(Restriction::Mute);
                self.manual_effects += 1;
            }
            Op::IssueCaptcha { owner } if self.source_current && !self.latest_not_spam => {
                self.captcha = CaptchaState::Pending;
                self.captcha_owner = Some(owner);
            }
            Op::ApplyCaptchaRestriction
                if self.captcha == CaptchaState::Pending && self.manual_restriction.is_none() =>
            {
                self.captcha_owned_restriction = true;
            }
            Op::SolveCaptcha { actor }
                if self.captcha == CaptchaState::Pending && self.captcha_owner == Some(actor) =>
            {
                self.captcha = CaptchaState::Passed;
                if self.captcha_owned_restriction {
                    self.captcha_owned_restriction = false;
                    self.captcha_releases += 1;
                }
            }
            Op::ExpireCaptcha if self.captcha == CaptchaState::Pending => {
                self.captcha = CaptchaState::Expired;
            }
            Op::UndoManualRestriction => self.manual_restriction = None,
            Op::TelegramUnknown => {
                if !self.uncertain_delivery {
                    self.uncertain_delivery = true;
                    self.effects_at_uncertain_delivery = Some(self.external_effects);
                }
            }
            Op::RetryTelegramEffect if !self.uncertain_delivery => self.external_effects += 1,
            Op::RetryTelegramEffect => {}
            Op::DuplicateEvent => self.duplicate_events += 1,
            Op::ModeratorDisconnected => {}
            _ => {}
        }
    }

    pub fn invariant_holds(&self) -> bool {
        (self.uncertain_delivery == self.effects_at_uncertain_delivery.is_some())
            && self
                .effects_at_uncertain_delivery
                .is_none_or(|effects| effects == self.external_effects)
            && (self.captcha != CaptchaState::Passed
                || (self.captcha_owner.is_some() && !self.captcha_owned_restriction))
    }
}

pub fn expected_manual_authorization(
    actor: Role,
    bot: Role,
    target: Role,
    target_is_bot: bool,
) -> bool {
    actor.can_restrict() && bot.can_restrict() && !target.is_privileged() && !target_is_bot
}
