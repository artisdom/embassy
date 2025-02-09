use crate::pac::rcc::vals::{Hpre, Pllm, Plln, Pllq, Pllr, Ppre, Sw};
use crate::pac::{FLASH, PWR, RCC};
use crate::rcc::{set_freqs, Clocks};
use crate::time::Hertz;

/// HSI speed
pub const HSI_FREQ: Hertz = Hertz(16_000_000);

/// Clocks configuration
#[non_exhaustive]
#[derive(Default)]
pub struct Config {
    pub hse: Option<Hertz>,
    pub bypass_hse: bool,
    pub hclk: Option<Hertz>,
    pub sys_ck: Option<Hertz>,
    pub pclk1: Option<Hertz>,
    pub pclk2: Option<Hertz>,

    #[cfg(not(any(stm32f410, stm32f411, stm32f412, stm32f413, stm32f423, stm32f446)))]
    pub plli2s: Option<Hertz>,

    #[cfg(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479))]
    pub pllsai: Option<Hertz>,

    pub pll48: bool,
    pub ls: super::LsConfig,
}

#[cfg(stm32f410)]
fn setup_i2s_pll(_vco_in: u32, _plli2s: Option<u32>) -> Option<u32> {
    None
}

// Not currently implemented, but will be in the future
#[cfg(any(stm32f411, stm32f412, stm32f413, stm32f423, stm32f446))]
fn setup_i2s_pll(_vco_in: u32, _plli2s: Option<u32>) -> Option<u32> {
    None
}

#[cfg(not(any(stm32f410, stm32f411, stm32f412, stm32f413, stm32f423)))]
fn calculate_sai_i2s_pll_values(vco_in: u32, max_div: u32, target: Option<u32>) -> Option<(u32, u32, u32)> {
    let min_div = 2;
    let target = match target {
        Some(target) => target,
        None => return None,
    };

    // We loop through the possible divider values to find the best configuration. Looping
    // through all possible "N" values would result in more iterations.
    let (n, outdiv, output, _error) = (min_div..=max_div)
        .filter_map(|outdiv| {
            let target_vco_out = match target.checked_mul(outdiv) {
                Some(x) => x,
                None => return None,
            };
            let n = (target_vco_out + (vco_in >> 1)) / vco_in;
            let vco_out = vco_in * n;
            if !(100_000_000..=432_000_000).contains(&vco_out) {
                return None;
            }
            let output = vco_out / outdiv;
            let error = (output as i32 - target as i32).unsigned_abs();
            Some((n, outdiv, output, error))
        })
        .min_by_key(|(_, _, _, error)| *error)?;

    Some((n, outdiv, output))
}

#[cfg(not(any(stm32f410, stm32f411, stm32f412, stm32f413, stm32f423, stm32f446)))]
fn setup_i2s_pll(vco_in: u32, plli2s: Option<u32>) -> Option<u32> {
    let (n, outdiv, output) = calculate_sai_i2s_pll_values(vco_in, 7, plli2s)?;

    RCC.plli2scfgr().modify(|w| {
        w.set_plli2sn(n as u16);
        w.set_plli2sr(outdiv as u8);
        #[cfg(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479))]
        w.set_plli2sq(outdiv as u8); //set sai divider same as i2s
    });

    Some(output)
}

#[cfg(not(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479)))]
fn setup_sai_pll(_vco_in: u32, _pllsai: Option<u32>) -> Option<u32> {
    None
}

#[cfg(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479))]
fn setup_sai_pll(vco_in: u32, pllsai: Option<u32>) -> Option<u32> {
    let (n, outdiv, output) = calculate_sai_i2s_pll_values(vco_in, 15, pllsai)?;

    RCC.pllsaicfgr().modify(|w| {
        w.set_pllsain(n as u16);
        w.set_pllsaiq(outdiv as u8);
    });

    Some(output)
}

fn setup_pll(
    pllsrcclk: u32,
    use_hse: bool,
    pllsysclk: Option<u32>,
    plli2s: Option<u32>,
    pllsai: Option<u32>,
    pll48clk: bool,
) -> PllResults {
    use crate::pac::rcc::vals::{Pllp, Pllsrc};

    let sysclk = pllsysclk.unwrap_or(pllsrcclk);
    if pllsysclk.is_none() && !pll48clk {
        RCC.pllcfgr().modify(|w| w.set_pllsrc(Pllsrc::from_bits(use_hse as u8)));

        return PllResults {
            use_pll: false,
            pllsysclk: None,
            pll48clk: None,
            plli2sclk: None,
            pllsaiclk: None,
        };
    }
    // Input divisor from PLL source clock, must result to frequency in
    // the range from 1 to 2 MHz
    let pllm_min = (pllsrcclk + 1_999_999) / 2_000_000;
    let pllm_max = pllsrcclk / 1_000_000;

    // Sysclk output divisor must be one of 2, 4, 6 or 8
    let sysclk_div = core::cmp::min(8, (432_000_000 / sysclk) & !1);

    let target_freq = if pll48clk { 48_000_000 } else { sysclk * sysclk_div };

    // Find the lowest pllm value that minimize the difference between
    // target frequency and the real vco_out frequency.
    let pllm = unwrap!((pllm_min..=pllm_max).min_by_key(|pllm| {
        let vco_in = pllsrcclk / pllm;
        let plln = target_freq / vco_in;
        target_freq - vco_in * plln
    }));

    let vco_in = pllsrcclk / pllm;
    assert!((1_000_000..=2_000_000).contains(&vco_in));

    // Main scaler, must result in >= 100MHz (>= 192MHz for F401)
    // and <= 432MHz, min 50, max 432
    let plln = if pll48clk {
        // try the different valid pllq according to the valid
        // main scaller values, and take the best
        let pllq = unwrap!((4..=9).min_by_key(|pllq| {
            let plln = 48_000_000 * pllq / vco_in;
            let pll48_diff = 48_000_000 - vco_in * plln / pllq;
            let sysclk_diff = (sysclk as i32 - (vco_in * plln / sysclk_div) as i32).abs();
            (pll48_diff, sysclk_diff)
        }));
        48_000_000 * pllq / vco_in
    } else {
        sysclk * sysclk_div / vco_in
    };

    let pllp = (sysclk_div / 2) - 1;

    let pllq = (vco_in * plln + 47_999_999) / 48_000_000;
    let real_pll48clk = vco_in * plln / pllq;

    RCC.pllcfgr().modify(|w| {
        w.set_pllm(Pllm::from_bits(pllm as u8));
        w.set_plln(Plln::from_bits(plln as u16));
        w.set_pllp(Pllp::from_bits(pllp as u8));
        w.set_pllq(Pllq::from_bits(pllq as u8));
        w.set_pllsrc(Pllsrc::from_bits(use_hse as u8));
        w.set_pllr(Pllr::from_bits(0));
    });

    let real_pllsysclk = vco_in * plln / sysclk_div;

    PllResults {
        use_pll: true,
        pllsysclk: Some(real_pllsysclk),
        pll48clk: if pll48clk { Some(real_pll48clk) } else { None },
        plli2sclk: setup_i2s_pll(vco_in, plli2s),
        pllsaiclk: setup_sai_pll(vco_in, pllsai),
    }
}

fn flash_setup(sysclk: u32) {
    use crate::pac::flash::vals::Latency;

    // Be conservative with voltage ranges
    const FLASH_LATENCY_STEP: u32 = 30_000_000;

    critical_section::with(|_| {
        FLASH
            .acr()
            .modify(|w| w.set_latency(Latency::from_bits(((sysclk - 1) / FLASH_LATENCY_STEP) as u8)));
    });
}

pub(crate) unsafe fn init(config: Config) {
    let pllsrcclk = config.hse.map(|hse| hse.0).unwrap_or(HSI_FREQ.0);
    let sysclk = config.sys_ck.map(|sys| sys.0).unwrap_or(pllsrcclk);
    let sysclk_on_pll = sysclk != pllsrcclk;

    let plls = setup_pll(
        pllsrcclk,
        config.hse.is_some(),
        if sysclk_on_pll { Some(sysclk) } else { None },
        #[cfg(not(any(stm32f410, stm32f411, stm32f412, stm32f413, stm32f423, stm32f446)))]
        config.plli2s.map(|i2s| i2s.0),
        #[cfg(any(stm32f410, stm32f411, stm32f412, stm32f413, stm32f423, stm32f446))]
        None,
        #[cfg(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479))]
        config.pllsai.map(|sai| sai.0),
        #[cfg(not(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479)))]
        None,
        config.pll48,
    );

    if config.pll48 {
        let freq = unwrap!(plls.pll48clk);

        assert!((max::PLL_48_CLK as i32 - freq as i32).abs() <= max::PLL_48_TOLERANCE as i32);
    }

    let sysclk = if sysclk_on_pll { unwrap!(plls.pllsysclk) } else { sysclk };

    // AHB prescaler
    let hclk = config.hclk.map(|h| h.0).unwrap_or(sysclk);
    let (hpre_bits, hpre_div) = match (sysclk + hclk - 1) / hclk {
        0 => unreachable!(),
        1 => (Hpre::DIV1, 1),
        2 => (Hpre::DIV2, 2),
        3..=5 => (Hpre::DIV4, 4),
        6..=11 => (Hpre::DIV8, 8),
        12..=39 => (Hpre::DIV16, 16),
        40..=95 => (Hpre::DIV64, 64),
        96..=191 => (Hpre::DIV128, 128),
        192..=383 => (Hpre::DIV256, 256),
        _ => (Hpre::DIV512, 512),
    };

    // Calculate real AHB clock
    let hclk = sysclk / hpre_div;

    let pclk1 = config
        .pclk1
        .map(|p| p.0)
        .unwrap_or_else(|| core::cmp::min(max::PCLK1_MAX, hclk));

    let (ppre1_bits, ppre1) = match (hclk + pclk1 - 1) / pclk1 {
        0 => unreachable!(),
        1 => (0b000, 1),
        2 => (0b100, 2),
        3..=5 => (0b101, 4),
        6..=11 => (0b110, 8),
        _ => (0b111, 16),
    };
    let timer_mul1 = if ppre1 == 1 { 1 } else { 2 };

    // Calculate real APB1 clock
    let pclk1 = hclk / ppre1;
    assert!(pclk1 <= max::PCLK1_MAX);

    let pclk2 = config
        .pclk2
        .map(|p| p.0)
        .unwrap_or_else(|| core::cmp::min(max::PCLK2_MAX, hclk));
    let (ppre2_bits, ppre2) = match (hclk + pclk2 - 1) / pclk2 {
        0 => unreachable!(),
        1 => (0b000, 1),
        2 => (0b100, 2),
        3..=5 => (0b101, 4),
        6..=11 => (0b110, 8),
        _ => (0b111, 16),
    };
    let timer_mul2 = if ppre2 == 1 { 1 } else { 2 };

    // Calculate real APB2 clock
    let pclk2 = hclk / ppre2;
    assert!(pclk2 <= max::PCLK2_MAX);

    flash_setup(sysclk);

    if config.hse.is_some() {
        RCC.cr().modify(|w| {
            w.set_hsebyp(config.bypass_hse);
            w.set_hseon(true);
        });
        while !RCC.cr().read().hserdy() {}
    }

    if plls.use_pll {
        RCC.cr().modify(|w| w.set_pllon(true));

        if hclk > max::HCLK_OVERDRIVE_FREQUENCY {
            PWR.cr1().modify(|w| w.set_oden(true));
            while !PWR.csr1().read().odrdy() {}

            PWR.cr1().modify(|w| w.set_odswen(true));
            while !PWR.csr1().read().odswrdy() {}
        }

        while !RCC.cr().read().pllrdy() {}
    }

    #[cfg(not(stm32f410))]
    if plls.plli2sclk.is_some() {
        RCC.cr().modify(|w| w.set_plli2son(true));

        while !RCC.cr().read().plli2srdy() {}
    }

    #[cfg(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479))]
    if plls.pllsaiclk.is_some() {
        RCC.cr().modify(|w| w.set_pllsaion(true));
        while !RCC.cr().read().pllsairdy() {}
    }

    RCC.cfgr().modify(|w| {
        w.set_ppre2(Ppre::from_bits(ppre2_bits));
        w.set_ppre1(Ppre::from_bits(ppre1_bits));
        w.set_hpre(hpre_bits);
    });

    // Wait for the new prescalers to kick in
    // "The clocks are divided with the new prescaler factor from 1 to 16 AHB cycles after write"
    cortex_m::asm::delay(16);

    RCC.cfgr().modify(|w| {
        w.set_sw(if sysclk_on_pll {
            Sw::PLL
        } else if config.hse.is_some() {
            Sw::HSE
        } else {
            Sw::HSI
        })
    });

    let rtc = config.ls.init();

    set_freqs(Clocks {
        sys: Hertz(sysclk),
        apb1: Hertz(pclk1),
        apb2: Hertz(pclk2),

        apb1_tim: Hertz(pclk1 * timer_mul1),
        apb2_tim: Hertz(pclk2 * timer_mul2),

        ahb1: Hertz(hclk),
        ahb2: Hertz(hclk),
        ahb3: Hertz(hclk),

        pll48: plls.pll48clk.map(Hertz),

        #[cfg(not(stm32f410))]
        plli2s: plls.plli2sclk.map(Hertz),

        #[cfg(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479))]
        pllsai: plls.pllsaiclk.map(Hertz),

        rtc,
    });
}

struct PllResults {
    use_pll: bool,
    pllsysclk: Option<u32>,
    pll48clk: Option<u32>,
    #[allow(dead_code)]
    plli2sclk: Option<u32>,
    #[allow(dead_code)]
    pllsaiclk: Option<u32>,
}

mod max {
    #[cfg(stm32f401)]
    pub(crate) const SYSCLK_MAX: u32 = 84_000_000;
    #[cfg(any(stm32f405, stm32f407, stm32f415, stm32f417,))]
    pub(crate) const SYSCLK_MAX: u32 = 168_000_000;
    #[cfg(any(stm32f410, stm32f411, stm32f412, stm32f413, stm32f423,))]
    pub(crate) const SYSCLK_MAX: u32 = 100_000_000;
    #[cfg(any(stm32f427, stm32f429, stm32f437, stm32f439, stm32f446, stm32f469, stm32f479,))]
    pub(crate) const SYSCLK_MAX: u32 = 180_000_000;

    pub(crate) const HCLK_OVERDRIVE_FREQUENCY: u32 = 168_000_000;

    pub(crate) const PCLK1_MAX: u32 = PCLK2_MAX / 2;

    #[cfg(any(stm32f401, stm32f410, stm32f411, stm32f412, stm32f413, stm32f423,))]
    pub(crate) const PCLK2_MAX: u32 = SYSCLK_MAX;
    #[cfg(not(any(stm32f401, stm32f410, stm32f411, stm32f412, stm32f413, stm32f423,)))]
    pub(crate) const PCLK2_MAX: u32 = SYSCLK_MAX / 2;

    pub(crate) const PLL_48_CLK: u32 = 48_000_000;
    pub(crate) const PLL_48_TOLERANCE: u32 = 120_000;
}
