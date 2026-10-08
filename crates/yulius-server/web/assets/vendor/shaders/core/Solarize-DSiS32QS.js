import { Ac as defineStd, Ml as p, Ss as isZero, ys as crosses, zt as solarize } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "Solarize",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Adjustments",
	description: "Inverts tones above a luminance threshold — a classic darkroom and photo effect",
	props: {
		threshold: {
			default: .5,
			description: "Luminance level above which colors are inverted. Pixels brighter than this threshold get flipped.",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Threshold",
				group: "Effect"
			}
		},
		strength: {
			default: 1,
			description: "Blend between original (0) and fully solarized (1)",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		}
	},
	effect: solarize({
		threshold: p("threshold"),
		strength: p("strength")
	}),
	identityWhen: isZero("strength"),
	missingChildMessage: "You must pass a child component into the Solarize shader."
});
var Solarize_default = componentDefinition;
export { componentDefinition as n, Solarize_default as t };
