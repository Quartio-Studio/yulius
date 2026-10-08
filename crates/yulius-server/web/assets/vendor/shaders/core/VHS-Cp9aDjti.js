import { Ac as defineStd, Ml as p, Nn as tapeWarp, Wt as beatPulse, Zt as chromaSmearTaps, sn as gatherStack } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "VHS",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Analog VHS tape with intermittent tape damage, chroma bleed, and per-scanline noise",
	props: {
		wobble: {
			default: 1,
			description: "Overall amount of tape damage — waves, creases, and head-switching noise. Bursts on and off organically over time.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .01,
				label: "Wobble",
				group: "Effect"
			}
		},
		scanlineNoise: {
			default: .6,
			description: "Per-scanline fine chroma/luma jitter. Adds the classic horizontal-streak detail.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Scanline Noise",
				group: "Effect"
			}
		},
		smear: {
			default: .2,
			description: "Horizontal chroma smear (color bleed) amount. Positive trails color to the right (classic VHS), negative trails it to the left.",
			ui: {
				type: ["range", "map"],
				min: -2,
				max: 2,
				step: .01,
				label: "Smear",
				group: "Effect"
			}
		},
		speed: {
			default: 1,
			description: "Animation speed of the tape effects.",
			ui: {
				type: "range",
				min: .1,
				max: 3,
				step: .1,
				label: "Speed",
				group: "Effect"
			}
		}
	},
	effect: gatherStack(chromaSmearTaps({
		warp: tapeWarp({
			wobble: p("wobble"),
			jitter: p("scanlineNoise"),
			speed: p("speed")
		}),
		smear: p("smear")
	}), [beatPulse({
		wobble: p("wobble"),
		speed: p("speed")
	})])
});
var VHS_default = componentDefinition;
export { componentDefinition as n, VHS_default as t };
