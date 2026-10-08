import { $i as scratchStreaks, Ac as defineStd, Ki as noiseTone, Ml as p, Wi as linearPair, Xi as rawPlane, zo as rampOver } from "./std-ClftfWpF.js";
import { n as noiseColorPropsAB, r as noiseSpeedProp } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "Scratches",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Fine hairline scratches, like a worn film or scratched surface",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		...noiseColorPropsAB(),
		scale: {
			default: 2,
			description: "Pattern scale (higher = more, finer scratches)",
			ui: {
				type: "range",
				min: -2,
				max: 5,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		thickness: {
			default: 1,
			description: "Thickness of the scratches (higher = bolder streaks)",
			ui: {
				type: "range",
				min: .2,
				max: 5,
				step: .01,
				label: "Thickness",
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
		},
		...noiseSpeedProp()
	},
	paint: rampOver(noiseTone(rawPlane(scratchStreaks({ thickness: p("thickness") }), {
		scale: p("scale"),
		seed: p("seed")
	})), linearPair(p("colorA"), p("colorB")))
});
var Scratches_default = componentDefinition;
export { componentDefinition as n, Scratches_default as t };
