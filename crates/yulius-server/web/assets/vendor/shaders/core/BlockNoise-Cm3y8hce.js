import { Ac as defineStd, Fo as noiseField, Ki as noiseTone, Ml as p, Vi as evolving, ea as seededPlane, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { i as noiseToneProps, r as noiseSpeedProp, t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "BlockNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Blocky value noise with soft cells that morph over time",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		...noiseColorProps(),
		scale: {
			default: 2,
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
		...noiseToneProps(),
		...noiseSpeedProp()
	},
	paint: rampOver(noiseTone(seededPlane(evolving(noiseField("value"), { rate: .3 }), {
		scale: p("scale"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var BlockNoise_default = componentDefinition;
export { componentDefinition as n, BlockNoise_default as t };
