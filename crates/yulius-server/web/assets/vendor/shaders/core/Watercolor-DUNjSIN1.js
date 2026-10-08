import { Ac as defineStd, J as wash, K as paperGrain, L as bleedWarp, Ml as p, U as kuwahara, ys as crosses } from "./std-ClftfWpF.js";
import { a as transformColor } from "./transformations-P50Uo_Ju.js";
var wet = bleedWarp({ amount: p("bleed") });
const componentDefinition = defineStd({
	name: "Watercolor",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Painterly watercolor look — Kuwahara flattening, pigment edge darkening, paper grain and bleeding",
	props: {
		radius: {
			default: 3,
			compileTime: true,
			description: "Brush size — radius of the painterly flattening",
			ui: {
				type: "range",
				min: 1,
				max: 8,
				step: 1,
				label: "Brush",
				group: "Effect",
				hidden: true
			}
		},
		bleed: {
			default: 1,
			recompile: crosses(0),
			description: "Amount the pigment bleeds and wobbles past hard edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .01,
				label: "Bleed",
				group: "Effect"
			}
		},
		strength: {
			default: 1,
			recompile: crosses(0),
			description: "Blend between the original image (0) and the full watercolor effect (1)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Strength",
				group: "Effect"
			}
		},
		paper: {
			default: .35,
			recompile: crosses(0),
			description: "Strength of the paper grain the pigment settles into",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .01,
				label: "Paper",
				group: "Effect"
			}
		},
		paperColor: {
			default: "#fbf7ec",
			transform: transformColor,
			description: "Tint of the paper grain",
			ui: {
				type: "color",
				label: "Paper Color",
				group: "Colors"
			}
		}
	},
	effect: wash({
		at: wet,
		brush: kuwahara({
			at: wet,
			radius: p("radius")
		}),
		grain: paperGrain({ amount: p("paper") }),
		paper: p("paper"),
		paperColor: p("paperColor"),
		strength: p("strength")
	})
});
var Watercolor_default = componentDefinition;
export { componentDefinition as n, Watercolor_default as t };
