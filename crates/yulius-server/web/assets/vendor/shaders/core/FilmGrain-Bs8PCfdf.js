import { $f as f32, Ac as defineStd, At as filmGrain, Ml as p, Ss as isZero, ys as crosses } from "./std-ClftfWpF.js";
const componentDefinition = defineStd({
	name: "FilmGrain",
	role: "filter",
	species: "pointwise",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Stylize",
	description: "Analog film grain texture overlay, weighted toward darker areas",
	extraFields: { animTime: {
		schema: f32,
		initial: 0
	} },
	props: {
		strength: {
			default: .5,
			description: "Intensity of the film grain noise",
			recompile: crosses(0),
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 1,
				step: .025,
				label: "Strength",
				group: "Effect"
			}
		},
		bias: {
			default: 2,
			description: "Concentrates grain in darker areas. Higher values focus grain more heavily on shadows; 0 applies grain uniformly.",
			ui: {
				type: "range",
				min: 0,
				max: 10,
				step: .1,
				label: "Bias",
				group: "Effect"
			}
		},
		animated: {
			default: false,
			description: "When enabled, the grain pattern changes each frame for a dynamic film effect",
			ui: {
				type: "checkbox",
				label: "Animated",
				group: "Effect"
			},
			transform: (value) => value ? 1 : 0
		}
	},
	effect: filmGrain({
		strength: p("strength"),
		bias: p("bias"),
		animated: p("animated")
	}),
	identityWhen: isZero("strength")
});
var FilmGrain_default = componentDefinition;
export { componentDefinition as n, FilmGrain_default as t };
