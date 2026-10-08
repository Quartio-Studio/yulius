import { ou as colorStopsPropConfig } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
function noiseColorProps() {
	return {
		colorA: {
			default: "#ffffff",
			description: "First color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#000000",
			description: "Second color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		colorSpace: {
			default: "linear",
			transform: transformColorSpace,
			compileTime: true,
			description: "Color space for color interpolation",
			ui: {
				type: "select",
				options: colorSpaceOptions,
				label: "Color Space",
				group: "Colors"
			}
		}
	};
}
function noiseColorPropsAB() {
	return {
		colorA: {
			default: "#ffffff",
			description: "First color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#000000",
			description: "Second color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		}
	};
}
function noiseToneProps() {
	return {
		contrast: {
			default: 0,
			description: "Pattern contrast (higher = sharper transitions)",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 5,
				step: .1,
				label: "Contrast",
				group: "Effect"
			}
		},
		balance: {
			default: 0,
			description: "Balance between colors (negative = more colorB, positive = more colorA)",
			ui: {
				type: ["range", "map"],
				min: -1,
				max: 1,
				step: .05,
				label: "Balance",
				group: "Effect"
			}
		},
		seed: {
			default: 0,
			description: "Random seed for pattern variation",
			ui: {
				type: "range",
				min: 0,
				max: 100,
				step: 1,
				label: "Seed",
				group: "Effect"
			}
		}
	};
}
function noiseSpeedProp(defaultValue = 1) {
	return { speed: {
		default: defaultValue,
		description: "Animation speed",
		ui: {
			type: "range",
			min: 0,
			max: 5,
			step: .1,
			label: "Speed",
			group: "Animation"
		}
	} };
}
export { noiseToneProps as i, noiseColorPropsAB as n, noiseSpeedProp as r, noiseColorProps as t };
