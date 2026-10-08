import { Ac as defineStd, Bi as erosionRidges, Ki as noiseTone, Ml as p, aa as unitized, ea as seededPlane, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { i as noiseToneProps, t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "ErosionNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Branching, hydraulic-erosion ridges carved into noise",
	acceptsUVContext: true,
	props: {
		...noiseColorProps(),
		scale: {
			default: 1.5,
			description: "Pattern scale (higher = larger patterns)",
			ui: {
				type: "range",
				min: -2,
				max: 5,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		...noiseToneProps()
	},
	paint: rampOver(noiseTone(seededPlane(unitized(erosionRidges()), {
		scale: p("scale"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var ErosionNoise_default = componentDefinition;
export { componentDefinition as n, ErosionNoise_default as t };
