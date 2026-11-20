//! Step-by-step instructions for entering recovery and DFU mode.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Family {
    IPhoneModern,
    IPhone7,
    IPhoneHome,
    IPadNoHome,
    IPadHome,
}

impl Family {
    pub const ALL: [Family; 5] = [
        Family::IPhoneModern,
        Family::IPhone7,
        Family::IPhoneHome,
        Family::IPadNoHome,
        Family::IPadHome,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Family::IPhoneModern => "iPhone 8 and later (incl. SE 2nd/3rd gen)",
            Family::IPhone7 => "iPhone 7 / 7 Plus",
            Family::IPhoneHome => "iPhone 6s and earlier (incl. SE 1st gen)",
            Family::IPadNoHome => "iPad without Home button",
            Family::IPadHome => "iPad with Home button",
        }
    }

    /// Guess the family from a product type such as `iPhone14,5` or `iPad13,18`.
    pub fn from_product(product: &str) -> Option<Family> {
        let major = |prefix: &str| -> Option<u32> {
            product
                .strip_prefix(prefix)?
                .split(',')
                .next()?
                .parse()
                .ok()
        };
        if let Some(m) = major("iPhone") {
            return Some(match m {
                10.. => Family::IPhoneModern,
                9 => Family::IPhone7,
                _ => Family::IPhoneHome,
            });
        }
        if let Some(m) = major("iPad") {
            return Some(if m == 8 || m >= 13 {
                Family::IPadNoHome
            } else {
                Family::IPadHome
            });
        }
        None
    }

    pub fn recovery_steps(self) -> &'static [&'static str] {
        match self {
            Family::IPhoneModern => &[
                "Connect the iPhone to this computer with a USB cable.",
                "Press and quickly release the volume up button.",
                "Press and quickly release the volume down button.",
                "Press and hold the side button. Keep holding it while the Apple logo appears.",
                "Release the button when the recovery mode screen (cable and computer) appears.",
            ],
            Family::IPhone7 => &[
                "Connect the iPhone to this computer with a USB cable.",
                "Press and hold the side button and the volume down button at the same time.",
                "Keep holding both buttons while the Apple logo appears.",
                "Release them when the recovery mode screen appears.",
            ],
            Family::IPhoneHome => &[
                "Connect the iPhone to this computer with a USB cable.",
                "Press and hold the Home button and the side (or top) button at the same time.",
                "Keep holding both buttons while the Apple logo appears.",
                "Release them when the recovery mode screen appears.",
            ],
            Family::IPadNoHome => &[
                "Connect the iPad to this computer with a USB cable.",
                "Press and quickly release the volume button closest to the top button.",
                "Press and quickly release the other volume button.",
                "Press and hold the top button. Keep holding it while the Apple logo appears.",
                "Release the button when the recovery mode screen appears.",
            ],
            Family::IPadHome => &[
                "Connect the iPad to this computer with a USB cable.",
                "Press and hold the Home button and the top (or side) button at the same time.",
                "Keep holding both buttons while the Apple logo appears.",
                "Release them when the recovery mode screen appears.",
            ],
        }
    }

    pub fn dfu_steps(self) -> &'static [&'static str] {
        match self {
            Family::IPhoneModern => &[
                "Connect the iPhone to this computer with a USB cable.",
                "Press and quickly release the volume up button.",
                "Press and quickly release the volume down button.",
                "Press and hold the side button until the screen turns black.",
                "Keep holding the side button and additionally press and hold volume down for 5 seconds.",
                "Release the side button, but keep holding volume down for about 10 more seconds.",
                "The screen stays black. Obsidia now shows the device in DFU mode.",
            ],
            Family::IPhone7 => &[
                "Connect the iPhone to this computer with a USB cable.",
                "Press and hold the side button and the volume down button together for 8 seconds.",
                "Release the side button, but keep holding volume down for about 5 more seconds.",
                "The screen stays black. Obsidia now shows the device in DFU mode.",
            ],
            Family::IPhoneHome => &[
                "Connect the iPhone to this computer with a USB cable.",
                "Press and hold the Home button and the side (or top) button together for 8 seconds.",
                "Release the side button, but keep holding the Home button for about 5 more seconds.",
                "The screen stays black. Obsidia now shows the device in DFU mode.",
            ],
            Family::IPadNoHome => &[
                "Connect the iPad to this computer with a USB cable.",
                "Press and quickly release the volume button closest to the top button.",
                "Press and quickly release the other volume button.",
                "Press and hold the top button until the screen turns black.",
                "Keep holding the top button and additionally press and hold volume down for 5 seconds.",
                "Release the top button, but keep holding volume down for about 10 more seconds.",
                "The screen stays black. Obsidia now shows the device in DFU mode.",
            ],
            Family::IPadHome => &[
                "Connect the iPad to this computer with a USB cable.",
                "Press and hold the Home button and the top button together for 8 seconds.",
                "Release the top button, but keep holding the Home button for about 5 more seconds.",
                "The screen stays black. Obsidia now shows the device in DFU mode.",
            ],
        }
    }

    pub fn force_restart(self) -> &'static str {
        match self {
            Family::IPhoneModern => {
                "Press and release volume up, press and release volume down, then hold the side button until the Apple logo appears."
            }
            Family::IPhone7 => {
                "Hold the side button and volume down together until the Apple logo appears."
            }
            Family::IPhoneHome => {
                "Hold the Home button and the side (or top) button together until the Apple logo appears."
            }
            Family::IPadNoHome => {
                "Press and release the volume button closest to the top button, then the other one, then hold the top button until the Apple logo appears."
            }
            Family::IPadHome => {
                "Hold the Home button and the top button together until the Apple logo appears."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_families() {
        assert_eq!(
            Family::from_product("iPhone14,5"),
            Some(Family::IPhoneModern)
        );
        assert_eq!(
            Family::from_product("iPhone12,8"),
            Some(Family::IPhoneModern)
        );
        assert_eq!(Family::from_product("iPhone9,3"), Some(Family::IPhone7));
        assert_eq!(Family::from_product("iPhone8,4"), Some(Family::IPhoneHome));
        assert_eq!(Family::from_product("iPad13,18"), Some(Family::IPadNoHome));
        assert_eq!(Family::from_product("iPad8,11"), Some(Family::IPadNoHome));
        assert_eq!(Family::from_product("iPad12,1"), Some(Family::IPadHome));
        assert_eq!(Family::from_product("iPad7,11"), Some(Family::IPadHome));
        assert_eq!(Family::from_product("Watch6,1"), None);
    }
}
