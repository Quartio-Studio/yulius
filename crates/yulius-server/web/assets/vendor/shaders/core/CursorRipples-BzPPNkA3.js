import { Ac as defineStd, As as op, Ds as pointerSpeed, Es as pointer, Ml as p, Rs as displaceBy, js as simulate } from "./std-ClftfWpF.js";
import { s as transformEdges } from "./transformations-P50Uo_Ju.js";
var waves = simulate.grid({
	resolution: 128,
	history: 2,
	step: [op.wave({ damping: p("decay") }), op.splat({
		at: pointer({ teleportGuard: "on" }),
		amount: pointerSpeed({ max: 2 }),
		radius: p("radius")
	})],
	derive: { displacement: op.gradient() },
	rest: { settlesWhen: "derived-from-damping" }
});
const componentDefinition = defineStd({
	name: "CursorRipples",
	role: "filter",
	species: "gather",
	boundingBoxDeclaration: { aspectRatio: null },
	category: "Interactive",
	description: "Fluid-like ripple distortion",
	props: {
		intensity: {
			default: 10,
			description: "Strength of the ripple distortion",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 20,
				step: .1,
				label: "Intensity",
				group: "Effect"
			}
		},
		decay: {
			default: 10,
			description: "How quickly ripples fade (higher = faster)",
			ui: {
				type: "range",
				min: 0,
				max: 20,
				step: .1,
				label: "Decay",
				group: "Effect"
			}
		},
		radius: {
			default: .5,
			description: "Radius of cursor influence",
			ui: {
				type: "range",
				min: .1,
				max: 1,
				step: .1,
				label: "Radius",
				group: "Effect"
			}
		},
		chromaticSplit: {
			default: 1,
			description: "RGB channel separation along ripple edges",
			ui: {
				type: ["range", "map"],
				min: 0,
				max: 3,
				step: .1,
				label: "Chromatic Split",
				group: "Effect"
			}
		},
		edges: {
			default: "stretch",
			description: "How to handle edges when distortion pushes content out of bounds",
			transform: transformEdges,
			compileTime: true,
			ui: {
				type: "select",
				options: [
					{
						label: "Stretch",
						value: "stretch"
					},
					{
						label: "Transparent",
						value: "transparent"
					},
					{
						label: "Mirror",
						value: "mirror"
					},
					{
						label: "Wrap",
						value: "wrap"
					}
				],
				label: "Edges",
				group: "Effect"
			}
		}
	},
	effect: displaceBy(waves.output("displacement"), {
		strength: p("intensity"),
		chromatic: p("chromaticSplit"),
		edges: p("edges")
	})
});
var CursorRipples_default = componentDefinition;
export { componentDefinition as n, CursorRipples_default as t };
