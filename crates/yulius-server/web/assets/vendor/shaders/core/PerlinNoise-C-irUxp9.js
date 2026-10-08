import { Ac as defineStd, Fo as noiseField, Ki as noiseTone, Ml as p, Vi as evolving, ea as seededPlane, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { i as noiseToneProps, r as noiseSpeedProp, t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "PerlinNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Smooth gradient noise that morphs over time",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		...noiseColorProps(),
		scale: {
			default: 2,
			description: "Pattern scale (higher = finer, more detailed patterns; each +1 roughly doubles the frequency)",
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
	paint: rampOver(noiseTone(seededPlane(evolving(noiseField("perlin"), { rate: .3 }), {
		scale: p("scale"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var PerlinNoise_default = componentDefinition;
export { componentDefinition as n, PerlinNoise_default as t };
