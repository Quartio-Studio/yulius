import { Ac as defineStd, Go as shimmered, Jo as threshold, Lo as pulse, Ml as p, Wo as share, jo as flowField, ou as colorStopsPropConfig, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { a as transformColor, o as transformColorSpace, t as colorSpaceOptions } from "./transformations-P50Uo_Ju.js";
var swirlPatternField = share(flowField({ detail: p("detail") }));
const componentDefinition = defineStd({
	name: "Swirl",
	role: "generator",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Textures",
	description: "Flowing swirl pattern with multi-layered noise",
	animatedTime: { speed: "speed" },
	props: {
		colorA: {
			default: "#1275d8",
			description: "Primary gradient color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color A",
				group: "Colors"
			}
		},
		colorB: {
			default: "#e19136",
			description: "Secondary gradient color",
			transform: transformColor,
			ui: {
				type: "color",
				label: "Color B",
				group: "Colors"
			}
		},
		stops: colorStopsPropConfig(),
		speed: {
			default: 1,
			description: "Flow animation speed",
			ui: {
				type: "range",
				min: 0,
				max: 5,
				step: .1,
				label: "Speed",
				group: "Animation"
			}
		},
		detail: {
			default: 1,
			description: "Level of detail and intricacy in the swirl patterns",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 5,
				step: .1,
				label: "Detail",
				group: "Effect"
			}
		},
		blend: {
			default: 50,
			description: "Skew color balance toward A (lower values) or B (higher values)",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 100,
				step: 1,
				label: "Blend",
				group: "Effect"
			}
		},
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
	},
	paint: shimmered(rampOver(threshold(swirlPatternField, {
		low: .3,
		high: .7,
		bias: {
			base: p("blend"),
			add: -50,
			mul: .006
		}
	}), stops(p("colorSpace"))), pulse(swirlPatternField, {
		speed: 2.5,
		span: 8,
		depth: .015
	}))
});
var Swirl_default = componentDefinition;
export { componentDefinition as n, Swirl_default as t };
